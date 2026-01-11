#![no_std]
#![no_main]

const SSID: &str = "ssid";
const PASSWORD: &str = "password";

use esp_backtrace as _;
use esp_println::{print, println};
use esp_radio::wifi;
use smoltcp::{iface, socket::dhcpv4, socket::tcp, time, wire};

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    println!("Firmware starting");

    let config = esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max());
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let mut led = esp_hal::gpio::Output::new(
        peripherals.GPIO15,
        esp_hal::gpio::Level::Low,
        esp_hal::gpio::OutputConfig::default(),
    );

    let timer_group = esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0);
    let sw_int =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timer_group.timer0, sw_int.software_interrupt0);

    // Wifi configuration
    let esp_radio_ctrl = esp_radio::init().unwrap();
    let (mut wifi_controller, interfaces) =
        esp_radio::wifi::new(&esp_radio_ctrl, peripherals.WIFI, Default::default()).unwrap();
    let mut wifi_device = interfaces.sta;
    wifi_controller
        .set_config(&wifi::ModeConfig::Client(
            wifi::ClientConfig::default()
                .with_ssid(SSID.into())
                .with_password(PASSWORD.into()),
        ))
        .unwrap();
    wifi_controller.start().unwrap();

    // Smoltcp configuration
    let mut interface = iface::Interface::new(
        iface::Config::new(wire::EthernetAddress::from_bytes(&wifi_device.mac_address()).into()),
        &mut wifi_device,
        time::Instant::from_micros(
            esp_hal::time::Instant::now()
                .duration_since_epoch()
                .as_micros() as i64,
        ),
    );
    const NUM_TCP_SOCKETS: usize = 3;
    let mut socket_storage: [iface::SocketStorage; 1 + NUM_TCP_SOCKETS] = Default::default();
    let mut sockets = iface::SocketSet::new(socket_storage.as_mut_slice());
    let dhcp_handle = sockets.add(dhcpv4::Socket::new());
    let mut tcp_buffers = [([0u8; 1024], [0u8; 1024]); NUM_TCP_SOCKETS];
    let mut tcp_connections: [TcpConnection; NUM_TCP_SOCKETS] = Default::default();
    for (tcp_connection, (rx_buffer, tx_buffer)) in
        tcp_connections.iter_mut().zip(tcp_buffers.iter_mut())
    {
        let mut tcp_socket = tcp::Socket::new(
            tcp::SocketBuffer::new(rx_buffer.as_mut_slice()),
            tcp::SocketBuffer::new(tx_buffer.as_mut_slice()),
        );
        tcp_socket.set_timeout(Some(time::Duration::from_millis(500)));
        tcp_socket.set_keep_alive(Some(time::Duration::from_millis(100)));
        tcp_connection.socket_handle = sockets.add(tcp_socket);
    }

    // Main loop
    loop {
        // 1. Connect to wifi if needed
        if !wifi_controller.is_connected().unwrap_or(false) {
            sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).reset();
            for tcp_connection in tcp_connections.iter_mut() {
                tcp_connection.reset(&mut sockets);
            }
            print!("Wifi connecting to network \"{}\"... ", SSID);
            while !wifi_controller.is_connected().unwrap_or(false) {
                let _ = wifi_controller.connect();
                // often returns an InternalError(EspErrWifiConn) a few times before succeeding
            }
            println!("connected!");
        }

        // 2. Update all smoltcp sockets
        interface.poll(
            time::Instant::from_micros(
                esp_hal::time::Instant::now()
                    .duration_since_epoch()
                    .as_micros() as i64,
            ),
            &mut wifi_device,
            &mut sockets,
        );
        dhcp_handler(
            sockets.get_mut::<dhcpv4::Socket>(dhcp_handle),
            &mut interface,
        );
        for tcp_connection in tcp_connections.iter_mut() {
            tcp_connection.handler(&mut sockets, |post_request| match post_request {
                b"led=on" => {
                    led.set_high();
                }
                b"led=off" => {
                    led.set_low();
                }
                _ => println!("Bad POST request"),
            });
        }
    }
}

fn dhcp_handler(socket: &mut dhcpv4::Socket, interface: &mut iface::Interface) {
    // See https://github.com/smoltcp-rs/smoltcp/blob/main/examples/dhcp_client.rs
    match socket.poll() {
        None => {}
        Some(dhcpv4::Event::Configured(config)) => {
            println!("DHCP configured with IP {}", config.address);
            interface.update_ip_addrs(|addrs| {
                addrs.clear();
                addrs.push(wire::IpCidr::Ipv4(config.address)).unwrap();
            });
            if let Some(router) = config.router {
                interface
                    .routes_mut()
                    .add_default_ipv4_route(router)
                    .unwrap();
            } else {
                interface.routes_mut().remove_default_ipv4_route();
            }
        }
        Some(dhcpv4::Event::Deconfigured) => {
            println!("DHCP deconfigured");
            interface.update_ip_addrs(|addrs| addrs.clear());
            interface.routes_mut().remove_default_ipv4_route();
        }
    }
}

struct TcpConnection {
    socket_handle: iface::SocketHandle,
    buffer: [u8; 1024],
    buffer_index: usize,
}

impl Default for TcpConnection {
    fn default() -> Self {
        Self {
            socket_handle: Default::default(),
            buffer: [0; 1024],
            buffer_index: 0,
        }
    }
}

impl TcpConnection {
    fn reset(&mut self, sockets: &mut iface::SocketSet) {
        sockets.get_mut::<tcp::Socket>(self.socket_handle).abort();
        self.buffer_index = 0;
    }
    fn handler(&mut self, sockets: &mut iface::SocketSet, mut post_handler: impl FnMut(&[u8])) {
        let socket = sockets.get_mut::<tcp::Socket>(self.socket_handle);
        if !socket.is_open() {
            self.buffer_index = 0;
            socket.listen(80).unwrap();
        }
        if !socket.can_recv() {
            // Nothing to do yet, since no data has been received
            return;
        }
        if let Ok(len) = socket.recv_slice(&mut self.buffer[self.buffer_index..]) {
            self.buffer_index += len;
        }
        // Try to parse the HTTP request.
        // Most modern web browsers leave the TCP connection open until the server closes it,
        // so parsing the request and looking at the Content-Length is the only way to tell
        // if it's time to respond.
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut http_request = httparse::Request::new(&mut headers);
        let parse_result = http_request.parse(&self.buffer[..self.buffer_index]);
        let num_bytes = if let Ok(httparse::Status::Complete(num_bytes)) = parse_result {
            num_bytes
        } else {
            // We have not yet received a complete HTTP request
            return;
        };
        let mut content_length = 0;
        // Extract the value of the Content-Length header
        for header in http_request.headers.iter() {
            if header.name == "Content-Length"
                && let Ok(value) = core::str::from_utf8(header.value)
                && let Ok(value) = value.parse()
            {
                content_length = value;
            }
        }
        if self.buffer_index < num_bytes + content_length {
            // We received a complete request header, but the body is still incomplete
            return;
        }
        let content = &self.buffer[num_bytes..][..content_length];
        // If execution reaches this point, we have received a complete HTTP request.
        println!(
            "Received HTTP request: {} {}, data={:?}",
            http_request.method.unwrap_or("unknown"),
            http_request.path.unwrap_or("unknown"),
            core::str::from_utf8(content).unwrap_or("invalid"),
        );
        if http_request.path == Some("/") {
            socket.send_slice(b"HTTP/1.0 200 OK\r\n\r\n").unwrap();
            socket.send_slice(include_bytes!("index.html")).unwrap();
            if http_request.method == Some("POST") {
                post_handler(content);
            }
        } else {
            socket
                .send_slice(b"HTTP/1.0 404 Not Found\r\n\r\n")
                .unwrap();
        }
        self.buffer_index = 0;
        socket.close();
    }
}
