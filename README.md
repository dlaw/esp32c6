# ESP32-C6 Rust example

This is a bare-metal Rust firmware which runs on the ESP32-C6.  There is no standard library and no operating system.  It should work with any ESP32-C6 board.  I recommend the [Beetle](https://www.dfrobot.com/product-2778.html) from DFRobot.

### Wifi

Wifi support is provided by [esp-wifi](https://github.com/esp-rs/esp-wifi/tree/main/esp-wifi).  This is a nicely Rust-ified wrapper around the C interface to the Espressif-provided binary blob.  The firmware connects to a pre-specified network, and automatically attempts to reconnect anytime the connection is lost.  Remember to substitute your network's SSID and password into `src/main.rs`.

### Networking

Unlike the esp-wifi examples, we interact with [smoltcp](https://github.com/smoltcp-rs/smoltcp) directly instead of using esp-wifi's [wifi_interface wrappers](https://github.com/esp-rs/esp-wifi/blob/main/esp-wifi/src/wifi_interface.rs). (The wrappers don't subtract much complexity, while at the same time they hide some very important details of smoltcp.)  The firmware is set up to get an ipv4 address over DHCP.

### LED

The firmware assumes that GPIO15 is attached to a LED, which matches the DFRobot schematic.  If you have a different LED situation, update `src/main.rs`.

### HTTP server

A small HTTP server parses HTTP requests and serves the page from `src/index.html`.  This is a small form with buttons which generate POST requests to turn the LED on or off.  Mainly, it's a demonstration that all the wifi and network stuff is working and ready for your actual application to be added.

The HTTP server sometimes gets confused when a modern browser such as Chrome preemptively opens 17 simultaenous TCP connections to load all the bullshit which it expects will encumber any modern website.  If this happens to you, I recommend using `curl` instead:
```bash
curl IP_OF_ESP32                # GET request
curl -d "led=on" IP_OF_ESP32    # POST request, turn on LED
curl -d "led=off" IP_OF_ESP32   # POST request, turn off LED
```

### Printing

The `esp-println` library is used to direct all printed output, as well as panic backtraces, to the serial interface of the Espressif USB peripheral.  This interface is automatically displayed by `espflash`, or it can be accessed like any other serial port.  On Linux it should appear at `/dev/serial/by-id/usb-Espressif_USB_JTAG_serial_debug_unit_*-if00`.

### Programming

Programming and debugging are performed over the JTAG interface of the USB peripheral.  There are two options: `cargo run` programs with `espflash`; `cargo embed` programs with `probe-rs`.