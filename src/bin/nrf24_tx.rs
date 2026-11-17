#![no_std]
#![no_main]

use core::fmt::Write;
use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use embassy_time::{Duration, Timer};
use heapless::String;
use nucleo_l432kc_app::nrf24::{Nrf24, TxError, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_stm32::init(Default::default());

    // Status / heartbeat LED on PB3 (onboard LD3 on Nucleo-32)
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    // CE (PB0, Arduino D3 on Nucleo-32)
    let ce = Output::new(p.PB0, Level::Low, Speed::Low);

    // CSN (PA11, Arduino D10 on Nucleo-32)
    let csn = Output::new(p.PA11, Level::High, Speed::High);

    // Give RTT a moment to attach before logging
    Timer::after(Duration::from_millis(500)).await;
    info!("===============================================");
    info!("=== STM32L432 NRF24L01 Transmitter (TX)     ===");
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

    // Configure as Transmitter
    radio.config_tx().await;
    info!("Transmitter configured! Starting periodic packet transmission...");

    let mut packet_num: u32 = 0;

    loop {
        packet_num += 1;
        led.set_high();

        // Build a 32-byte payload containing a readable message
        let mut payload = [0u8; PAYLOAD_SIZE];
        let mut msg: String<32> = String::new();
        let _ = write!(msg, "PING #{:<6} STM32", packet_num);
        let bytes = msg.as_bytes();
        payload[..bytes.len()].copy_from_slice(bytes);

        info!("Sending packet #{}: \"{}\"", packet_num, msg.as_str());

        match radio.send_packet(&payload).await {
            Ok(()) => {
                info!(
                    "--> SUCCESS: Packet #{} transmitted and ACK confirmed by receiver!",
                    packet_num
                );
            }
            Err(TxError::MaxRetries) => {
                warn!(
                    "--> Transmitted packet #{}, but NO ACK received! (Ensure RX board is powered on, running nrf24_rx)",
                    packet_num
                );
            }
            Err(TxError::Timeout) => {
                error!("--> Transmission timed out!");
            }
        }

        led.set_low();
        Timer::after(Duration::from_millis(1000)).await;
    }
}
