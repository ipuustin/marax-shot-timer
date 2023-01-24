use embedded_graphics::{
    egtext, pixelcolor::BinaryColor, prelude::*, text_style
};
use linux_embedded_hal::I2cdev;
use ssd1306::prelude::I2CInterface;
use ssd1306::{mode::GraphicsMode, Builder, I2CDIBuilder};
extern crate ctrlc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time;

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

async fn run_pump(mut disp: GraphicsMode<I2CInterface<I2cdev>>) -> u8 {
    return 0;
}

#[tokio::main]
async fn main() {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    let i2c = I2cdev::new("/dev/i2c-1").unwrap();

    let interface = I2CDIBuilder::new().init(i2c);
    let mut disp: GraphicsMode<I2CInterface<I2cdev>> = Builder::new().connect(interface).into();

    disp.init().unwrap();
    disp.flush().unwrap();

    let first_digit_position = Point::new(30, 22);
    let second_digit_position = Point::new(67, 22);

    let mut draw_number = |first_digit: u8, second_digit: u8| {
        disp.clear();

        if first_digit != 0 {
            egtext!(
                text = &first_digit.to_string(),
                top_left = first_digit_position,
                style = text_style!(font = SevenSegmentFont, text_color = BinaryColor::On)
            )
            .draw(&mut disp).unwrap();
        }
        egtext!(
            text = &second_digit.to_string(),
            top_left = second_digit_position,
            style = text_style!(font = SevenSegmentFont, text_color = BinaryColor::On)
        )
        .draw(&mut disp).unwrap();
        disp.flush().unwrap();
    };

    let mut interval = time::interval(time::Duration::from_secs(1));
    for _i in 1..99 {
        interval.tick().await;
        draw_number(_i / 10, _i % 10);
    }

    disp.clear();
    disp.flush().unwrap();
}
