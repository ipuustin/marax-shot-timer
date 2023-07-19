use embedded_graphics::{egtext, pixelcolor::BinaryColor, prelude::*, text_style};
use linux_embedded_hal::I2cdev;
use ssd1306::prelude::I2CInterface;
use ssd1306::{mode::GraphicsMode, Builder, I2CDIBuilder};

use bytes::BytesMut;
use futures::stream::StreamExt;

use prometheus::{IntGauge, Opts, Registry};
use prometheus_hyper::{RegistryFn, Server};

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{error::Error, io, net::SocketAddr, str};

use tokio::sync::Notify;
use tokio::time;
use tokio_serial::SerialPortBuilderExt;
use tokio_util::codec::{Decoder, Encoder};

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

        // Clean up after the timer is done. TODO: should we keep the last value visible for a while?
        disp.clear();
        disp.flush().unwrap();
    }

    // Clean up before exit.
    disp.clear();
    disp.flush().unwrap();
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
    pub machine_mode: IntGauge,
    pub steam_temperature: IntGauge,
    pub target_steam_temperature: IntGauge,
    pub hx_temperature: IntGauge,
    pub countdown_boost_mode: IntGauge,
    pub heating_element_on: IntGauge,
    pub pump_on: IntGauge,
}

impl MaraXMetrics {
    pub fn new() -> Result<(Self, RegistryFn), Box<dyn Error>> {
        let machine_mode = IntGauge::with_opts(Opts::new(
            "MachineMode",
            "Machine mode: coffee (1) or steam (0)",
        ))?;
        let machine_mode_clone = machine_mode.clone();

        let steam_temperature =
            IntGauge::with_opts(Opts::new("SteamTemperature", "Boiler steam temperature"))?;
        let steam_temperature_clone = steam_temperature.clone();

        let target_steam_temperature = IntGauge::with_opts(Opts::new(
            "TargetSteamTemperature",
            "Boiler target steam temperature",
        ))?;
        let target_steam_temperature_clone = target_steam_temperature.clone();

        let hx_temperature =
            IntGauge::with_opts(Opts::new("HXTemperature", "Heat exchanger temperature"))?;
        let hx_temperature_clone = hx_temperature.clone();

        let countdown_boost_mode = IntGauge::with_opts(Opts::new(
            "CountdownBoostMode",
            "Countdown for exiting boost mode",
        ))?;
        let countdown_boost_mode_clone = countdown_boost_mode.clone();

        let heating_element_on = IntGauge::with_opts(Opts::new(
            "HeatingElementOn",
            "Heating element on (1) or off (0)",
        ))?;
        let heating_element_on_clone = heating_element_on.clone();

        let pump_on = IntGauge::with_opts(Opts::new("PumpOn", "Pump on (1) or off (0)"))?;
        let pump_on_clone = pump_on.clone();

        let f = |r: &Registry| -> Result<(), prometheus::Error> {
            r.register(Box::new(machine_mode_clone))?;
            r.register(Box::new(steam_temperature_clone))?;
            r.register(Box::new(target_steam_temperature_clone))?;
            r.register(Box::new(hx_temperature_clone))?;
            r.register(Box::new(countdown_boost_mode_clone))?;
            r.register(Box::new(heating_element_on_clone))?;
            r.register(Box::new(pump_on_clone))?;
            Ok(())
        };

        Ok((
            Self {
                machine_mode,
                steam_temperature,
                target_steam_temperature,
                hx_temperature,
                countdown_boost_mode,
                heating_element_on,
                pump_on,
            },
            Box::new(f),
        ))
    }
}

fn parse_line_and_update_metrics(
    line: &str,
    metrics: &MaraXMetrics,
) -> Result<bool, Box<dyn Error>> {
    // "C1.19,116,124,095,0560,0,0"

    let v: Vec<&str> = line.split(',').collect();

    if v.len() != 7 {
        return Err("parse error: wrong number of tokens")?;
    }

    if v[0].is_empty() {
        return Err("parse error: empty token 0")?;
    }

    match v[0].chars().next() {
        None => return Err("parse error: index out of range")?,
        Some(c) => match c {
            'C' => metrics.machine_mode.set(1),
            'V' => metrics.machine_mode.set(0),
            _ => return Err("parse error: unknown machine mode")?,
        },
    }

    let steam_temperature = v[1].parse::<i64>()?;
    metrics.steam_temperature.set(steam_temperature);

    let target_steam_temperature = v[2].parse::<i64>()?;
    metrics
        .target_steam_temperature
        .set(target_steam_temperature);

    let hx_temperature = v[3].parse::<i64>()?;
    metrics.hx_temperature.set(hx_temperature);

    let countdown_boost_mode = v[4].parse::<i64>()?;
    metrics.countdown_boost_mode.set(countdown_boost_mode);

    let heating_element_on = v[5].parse::<i64>()?;
    if heating_element_on != 0 && heating_element_on != 1 {
        return Err("parse error: wrong heating element state value")?;
    }
    metrics.heating_element_on.set(heating_element_on);

    let pump_on = v[6].parse::<i64>()?;
    if pump_on != 0 && pump_on != 1 {
        return Err("parse error: wrong pump state value")?;
    }
    metrics.pump_on.set(pump_on);

    Ok(pump_on == 1)
}

#[tokio::main]
async fn main() {
    let pump_running = Arc::new(AtomicBool::new(false));
    let pump_running_clone = pump_running.clone();

    let start_pump = Arc::new(Notify::new());
    let start_pump_clone = Arc::clone(&start_pump);
    let start_pump_clone_ctrlc = Arc::clone(&start_pump);

    let shutdown_prometheus = Arc::new(Notify::new());
    let shutdown_prometheus_clone = Arc::clone(&shutdown_prometheus);

    let pump_loop_exit = Arc::new(AtomicBool::new(false));
    let pump_loop_exit_clone = pump_loop_exit.clone();

    ctrlc::set_handler(move || {
        pump_loop_exit.store(true, Ordering::SeqCst);
        start_pump_clone_ctrlc.notify_one();
        shutdown_prometheus.notify_one();
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

    let _prometheus_handle = tokio::spawn(async move {
        Server::run(
            Arc::clone(&registry),
            SocketAddr::from(([0; 4], 8081)),
            shutdown_prometheus_clone.notified(),
        )
        .await
    });

    let _serial_handle = tokio::spawn(async move {
        while let Some(line_result) = reader.next().await {
            let line = line_result.expect("Failed to read line");
            println!("{}", line);
            // Parse the line we read from Mara X.

            let pump_was_running = pump_running.load(Ordering::SeqCst);
            match parse_line_and_update_metrics(&line, &metrics) {
                Ok(pump_on) => {
                    pump_running.store(pump_on, Ordering::SeqCst);

                    if pump_on && !pump_was_running {
                        start_pump.notify_one();
                    }
                }
                _ => println!("Couldn't parse line: {}", line),
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

    // Let the pump function control the server shutdown, so that we leave
    // the screen in a known state.
    _pump_handle.await.unwrap();
}
