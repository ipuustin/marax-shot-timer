# Lelit Mara X shot timer

This project is a timer for Lelit Mara X espresso machine, meant to be run on an
Raspberry Pi. It exposes the temperature and other data which Mara X provides
over serial bus to a Prometheus endpoint, making it possible to graph the
current espresso machine status with Grafana or similar visualization framework.
If there is a standard SSD1306 display connected to the Raspberry Pi I2C bus,
the espresso timer is shown on the display.

## Cross build

First, install arm-linux-gnueabi-hf toolchain. The instructions on how to do
this depend on the Linux distribution you are using.

Second, install Rust cross compilation target:

    $ rustup target add armv7-unknown-linux-gnueabihf

Finally, build the binary:

    $ cargo build --release --target armv7-unknown-linux-gnueabihf

Note that the toolchain depends on the distribution you are running on the
Raspberry Pi. If unsure, just do a native build on the Raspberry.
