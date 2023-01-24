use embedded_graphics::{egtext, pixelcolor::BinaryColor, prelude::*, text_style};
use linux_embedded_hal::I2cdev;
use ssd1306::prelude::I2CInterface;
use ssd1306::{mode::GraphicsMode, Builder, I2CDIBuilder};
extern crate ctrlc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;
use tokio::time;

use prometheus::{IntCounter, Opts, Registry};
use prometheus_hyper::{RegistryFn, Server};
use std::{error::Error, io, net::SocketAddr, str};

use tokio_util::codec::{Decoder, Encoder};

use bytes::BytesMut;
use tokio_serial::SerialPortBuilderExt;

use futures::stream::StreamExt;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
struct SevenSegmentFont;

impl Font for SevenSegmentFont {
    const FONT_IMAGE: &'static [u8] = include_bytes!("../assets/seven-segment-font.raw");
    const FONT_IMAGE_WIDTH: u32 = 224;

    const CHARACTER_SIZE: Size = Size::new(22, 40);
    const CHARACTER_SPACING: u32 = 4;

    fn char_offset(c: char) -> u32 {
        c.to_digit(10).unwrap_or(0)
    }
}

async fn run_pump(
    mut disp: GraphicsMode<I2CInterface<I2cdev>>,
    start_pump: Arc<Notify>,
    pump_running: Arc<AtomicBool>,
    exit: Arc<AtomicBool>,
) {
    let first_digit_position = Point::new(30, 22);
    let second_digit_position = Point::new(67, 22);

    let mut interval = time::interval(time::Duration::from_secs(1));

    loop {
        start_pump.notified().await;

        if exit.load(Ordering::SeqCst) {
            break;
        }

        for _i in 0..99 {
            if !pump_running.load(Ordering::SeqCst) {
                break;
            }

            let first_digit = _i / 10;
            let second_digit = _i % 10;

            disp.clear();

            if first_digit != 0 {
                egtext!(
                    text = &first_digit.to_string(),
                    top_left = first_digit_position,
                    style = text_style!(font = SevenSegmentFont, text_color = BinaryColor::On)
                )
                .draw(&mut disp)
                .unwrap();
            }
            egtext!(
                text = &second_digit.to_string(),
                top_left = second_digit_position,
                style = text_style!(font = SevenSegmentFont, text_color = BinaryColor::On)
            )
            .draw(&mut disp)
            .unwrap();

            disp.flush().unwrap();

            interval.tick().await;
        }

        disp.clear();
        disp.flush().unwrap();
    }
}

// Serial port codec implementation
struct LineCodec;

impl Decoder for LineCodec {
    type Item = String;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let newline = src.as_ref().iter().position(|b| *b == b'\n');
        if let Some(n) = newline {
            let line = src.split_to(n + 1);
            return match str::from_utf8(line.as_ref()) {
                Ok(s) => Ok(Some(s.to_string())),
                Err(_) => Err(io::Error::new(io::ErrorKind::Other, "Invalid String")),
            };
        }
        Ok(None)
    }
}

impl Encoder<String> for LineCodec {
    type Error = io::Error;

    fn encode(&mut self, _item: String, _dst: &mut BytesMut) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub struct MaraXMetrics {
    pub steam_temperature: IntCounter,
}

impl MaraXMetrics {
    pub fn new() -> Result<(Self, RegistryFn), Box<dyn Error>> {
        let steam_temperature =
            IntCounter::with_opts(Opts::new("SteamTemperature", "Boiler steam temperature"))?;
        let steam_temperature_clone = steam_temperature.clone();
        let f = |r: &Registry| r.register(Box::new(steam_temperature_clone));
        Ok((Self { steam_temperature }, Box::new(f)))
    }
}

fn parse_line(_line: String) -> bool {
    return false;
}

#[tokio::main]
async fn main() {
    let pump_running = Arc::new(AtomicBool::new(false));
    let pump_running_clone = pump_running.clone();

    let start_pump = Arc::new(Notify::new());
    let start_pump_clone = Arc::clone(&start_pump);
    let start_pump_clone_ctrlc = Arc::clone(&start_pump);

    let shutdown = Arc::new(Notify::new());
    let shutdown_clone = Arc::clone(&shutdown);

    let pump_loop_exit = Arc::new(AtomicBool::new(false));
    let pump_loop_exit_clone = pump_loop_exit.clone();

    ctrlc::set_handler(move || {
        pump_loop_exit.store(true, Ordering::SeqCst);
        start_pump_clone_ctrlc.notify_one();
        shutdown.notify_one();
    })
    .expect("Error setting Ctrl-C handler");

    // Initialize display

    let i2c = I2cdev::new("/dev/i2c-1").unwrap();

    let interface = I2CDIBuilder::new().init(i2c);
    let mut disp: GraphicsMode<I2CInterface<I2cdev>> = Builder::new().connect(interface).into();

    disp.init().unwrap();
    disp.flush().unwrap();

    // Start listening for Mara X serial events

    let mut serial_port = tokio_serial::new("/dev/ttyS0", 9600)
        .open_native_async()
        .unwrap();
    serial_port
        .set_exclusive(false)
        .expect("Unable to set serial port exclusive to false");
    let mut reader = LineCodec.framed(serial_port);

    // Start publishing Mara X values to the Prometheus endpoint

    let registry = Arc::new(Registry::new());
    let (metrics, f) = MaraXMetrics::new().expect("Failed prometheus metrics.");
    f(&registry).expect("Failed registering the registry.");

    let prometheus_handle = tokio::spawn(async move {
        Server::run(
            Arc::clone(&registry),
            SocketAddr::from(([0; 4], 8081)),
            shutdown_clone.notified(),
        )
        .await
    });

    metrics.steam_temperature.inc();

    let _serial_handle = tokio::spawn(async move {
        while let Some(line_result) = reader.next().await {
            let line = line_result.expect("Failed to read line");
            println!("{}", line);
            // Parse the line we read from Mara X.

            let pump_was_running = pump_running.load(Ordering::SeqCst);
            let pump_on = parse_line(line);
            pump_running.store(pump_on, Ordering::SeqCst);

            if pump_on && !pump_was_running {
                start_pump.notify_one();
            }
        }
    });

    let _pump_handle = tokio::spawn(async move {
        run_pump(
            disp,
            start_pump_clone,
            pump_running_clone,
            pump_loop_exit_clone,
        )
        .await
    });

    // Let the Prometheus server control the server shutdown.
    prometheus_handle.await.unwrap().unwrap();
}
