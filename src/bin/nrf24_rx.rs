#![no_std]
#![no_main]

use core::str;
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use embassy_time::{Duration, Timer};
use nucleo_l432kc_app::nrf24::{Nrf24, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());

    // Status / packet received LED on PB3 (onboard LD3 on Nucleo-32)
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    // CE (PA8, Arduino D9 on Nucleo-32)
    let ce = Output::new(p.PA8, Level::Low, Speed::Low);

    // CSN (PA11, Arduino D10 on Nucleo-32)
    let csn = Output::new(p.PA11, Level::High, Speed::High);

    // Give RTT a moment to attach before logging
    Timer::after(Duration::from_millis(500)).await;
    info!("===============================================");
    info!("=== STM32L432 NRF24L01 Receiver (RX)        ===");
    info!("===============================================");

    // SPI1: SCK = PA5 (A4), MOSI = PA7 (A6), MISO = PA6 (A5)
    let mut spi_config = Config::default();
    spi_config.frequency = Hertz(1_000_000);
    let spi = Spi::new_blocking(p.SPI1, p.PA5, p.PA7, p.PA6, spi_config);

    let mut radio = Nrf24::new(spi, ce, csn);

    // Initialize radio
    if !radio.init().await {
        error!("Failed to initialize NRF24L01! Check wiring, 3.3V power, and GND.");
        loop {
            led.set_high();
            Timer::after(Duration::from_millis(100)).await;
            led.set_low();
            Timer::after(Duration::from_millis(100)).await;
        }
    }

    // Configure as Receiver and start listening
    radio.config_rx().await;
    info!("Receiver listening on Channel 76 (2476 MHz)... Waiting for packets!");

    let mut rx_count: u32 = 0;
    let mut payload = [0u8; PAYLOAD_SIZE];

    loop {
        if radio.read_packet(&mut payload) {
            rx_count += 1;
            led.set_high();

            // Try to interpret payload as UTF-8 string up to the first null byte
            let end_idx = payload.iter().position(|&b| b == 0).unwrap_or(PAYLOAD_SIZE);
            let text = str::from_utf8(&payload[..end_idx]).unwrap_or("<binary data>");

            info!("==================================================");
            info!("--> RECEIVED Packet #{}: \"{}\"", rx_count, text);
            info!("    Raw bytes (32): {=[u8]:02x}", payload);
            info!("==================================================");

            // Keep LED lit briefly to show activity
            Timer::after(Duration::from_millis(50)).await;
            led.set_low();
        }

        // Small polling delay to yield to embassy executor
        Timer::after(Duration::from_millis(5)).await;
    }
}
