#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Initialize peripherals with default MSI oscillator clock
    let p = embassy_stm32::init(Default::default());
    info!("STM32L432KC initialized!");

    // External LED connected to D12 (PB4) on Nucleo-32.
    // Wire: PB4 -> series resistor (~330 ohm) -> LED anode, LED cathode -> GND.
    let mut led = Output::new(p.PB4, Level::Low, Speed::Low);

    loop {
        led.set_high();
        Timer::after(Duration::from_millis(500)).await;

        led.set_low();
        Timer::after(Duration::from_millis(500)).await;
    }
}
