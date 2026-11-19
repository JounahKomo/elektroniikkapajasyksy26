#![no_std]
#![no_main]

//! NRF24L01 audio receiver test: listens for audio packets and plays them on the speaker.
//!
//! Packet format (matches the `audio/*.raw` library): every 32-byte payload carries
//! 32 samples of 8-bit unsigned PCM (silence = 128) at 8,000 Hz, i.e. 4 ms of audio.
//! Any transmitter that sends such packets on channel 76 / address E7:E7:E7:E7:E7 works
//! (for example `nrf24_audio_file_tx` from the transmitter project).
//!
//! Radio (SPI1): SCK = PA5, MISO = PA6, MOSI = PA7, CSN = PA11, CE = PB0.
//! Speaker (I2S): BCLK = PA3, LRC = PA4, DIN = PA10.
//!
//! The I2S clocks are bit-banged by the CPU and must run continuously and evenly: pauses or
//! jitter in them leave the MAX98357A amplifier silent. So the radio SPI transfers run on
//! DMA, in the background, while I2S frames keep being clocked. The CPU only starts a
//! transfer and checks for completion between frames, and it talks to the radio just once
//! every `POLL_INTERVAL` frames. A jitter buffer absorbs packet timing variation: playback
//! starts once `PREBUFFER` samples have arrived and re-buffers after an underrun.

use core::pin::pin;
use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_futures::poll_once;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::mode::Async;
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use nucleo_l432kc_app::max98357a::{config_80mhz, Max98357a};
use nucleo_l432kc_app::nrf24::{Nrf24, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

/// Jitter buffer capacity in samples (must be a power of two): 2048 = 256 ms.
const BUF_SIZE: usize = 2048;
/// Samples to collect before playback (re)starts: 512 = 64 ms.
const PREBUFFER: usize = 512;
/// Playback volume shift (0 = unchanged, 1 = 2x, ...). Samples are clipped.
const VOLUME_SHIFT: u32 = 0;
/// I2S frames (62.5 us each) between two radio polls: 8 = 0.5 ms.
const POLL_INTERVAL: u32 = 8;

const CMD_NOP: u8 = 0xFF;
const CMD_R_RX_PAYLOAD: u8 = 0x61;

/// Playback state: jitter buffer plus the 8 kHz -> 16 kHz sample feeder and statistics.
struct Player {
    data: [u8; BUF_SIZE],
    head: usize,
    count: usize,
    playing: bool,
    sample: i16,
    frames: u32,
    peak: u16,
    packets: u32,
    underruns: u32,
    overruns: u32,
    led: Output<'static>,
}

impl Player {
    /// Append a packet; counts an overrun (and drops it) if it does not fit.
    fn push(&mut self, samples: &[u8]) {
        self.packets += 1;
        if self.count + samples.len() > BUF_SIZE {
            self.overruns += 1;
            return;
        }
        for &s in samples {
            self.data[(self.head + self.count) & (BUF_SIZE - 1)] = s;
            self.count += 1;
        }
    }

    fn pop(&mut self) -> u8 {
        let s = self.data[self.head];
        self.head = (self.head + 1) & (BUF_SIZE - 1);
        self.count -= 1;
        s
    }

    /// Clock exactly one I2S frame. Each 8 kHz sample is sent in two consecutive 16 kHz frames.
    #[inline(always)]
    fn frame(&mut self, audio: &mut Max98357a) {
        if self.frames & 1 == 0 {
            if !self.playing && self.count >= PREBUFFER {
                self.playing = true;
                self.led.set_high();
            }
            if self.playing && self.count == 0 {
                self.playing = false;
                self.underruns += 1;
                self.led.set_low();
            }
            self.sample = if self.playing {
                let v = ((self.pop() as i32) - 128) << (8 + VOLUME_SHIFT);
                v.clamp(-32768, 32767) as i16
            } else {
                0
            };
            self.peak = self.peak.max(self.sample.unsigned_abs());
        }

        let _ = audio.exchange_frame(self.sample as u16, self.sample as u16);

        self.frames += 1;
        if self.frames >= 16_000 {
            info!(
                "Packets: {}, buffered: {} samples, peak: {}, underruns: {}, overruns: {}, late bits: {}",
                self.packets,
                self.count,
                self.peak,
                self.underruns,
                self.overruns,
                audio.late_bits()
            );
            self.packets = 0;
            self.peak = 0;
            self.frames = 0;
        }
    }
}

/// Run one SPI transaction on DMA (CSN low for its duration) while I2S frames keep playing.
/// The first poll starts the DMA; a short transfer is finished by the time one frame is done.
fn spi_job(
    spi: &mut Spi<'static, Async>,
    csn: &mut Output<'static>,
    tx: &[u8],
    rx: &mut [u8],
    player: &mut Player,
    audio: &mut Max98357a,
) {
    csn.set_low();
    {
        let mut fut = pin!(spi.transfer(rx, tx));
        loop {
            if poll_once(fut.as_mut()).is_ready() {
                break;
            }
            player.frame(audio);
        }
    }
    csn.set_high();
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 80 MHz SYSCLK via PLL for the cycle-accurate I2S timing
    let mut p = embassy_stm32::init(config_80mhz());

    // Onboard LED LD3: lit while audio is being played
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);

    info!("=======================================================");
    info!("=== NRF24L01 Audio Receiver -> MAX98357A Speaker    ===");
    info!("=======================================================");
    info!("Expecting 32-byte packets of 8-bit unsigned PCM @ 8 kHz on channel 76.");

    let mut spi_config = Config::default();
    spi_config.frequency = Hertz(5_000_000);

    // Radio setup with the blocking driver. The peripherals are only borrowed so they can be
    // reused for the DMA-driven SPI afterwards.
    {
        let spi = Spi::new_blocking(&mut p.SPI1, &mut p.PA5, &mut p.PA7, &mut p.PA6, spi_config);
        let ce = Output::new(&mut p.PB0, Level::Low, Speed::Low);
        let csn = Output::new(&mut p.PA11, Level::High, Speed::High);
        let mut radio = Nrf24::new(spi, ce, csn);

        if !radio.init().await {
            error!("Failed to initialize NRF24L01! Check wiring, 3.3V power, and GND.");
            loop {
                led.toggle();
                embassy_time::Timer::after_millis(100).await;
            }
        }
        radio.config_rx().await;
    }
    info!("Radio listening on channel 76.");

    // CE must stay high for the receiver to listen (re-created right after the setup block)
    let _ce = Output::new(p.PB0, Level::High, Speed::Low);
    let mut csn = Output::new(p.PA11, Level::High, Speed::High);
    let mut spi = Spi::new(
        p.SPI1, p.PA5, p.PA7, p.PA6, p.DMA1_CH3, p.DMA1_CH2, spi_config,
    );

    let mut audio = Max98357a::new(p.PA3, p.PA4, p.PA10);

    // Startup beep: confirms the speaker path works before any radio audio arrives
    audio.play_tone(1000, 150);

    let mut player = Player {
        data: [128; BUF_SIZE],
        head: 0,
        count: 0,
        playing: false,
        sample: 0,
        frames: 0,
        peak: 0,
        packets: 0,
        underruns: 0,
        overruns: 0,
        led,
    };

    let mut tx = [CMD_NOP; PAYLOAD_SIZE + 1];
    let mut rx = [0u8; PAYLOAD_SIZE + 1];

    loop {
        // Play audio frames with no radio activity at all in between
        for _ in 0..POLL_INTERVAL {
            player.frame(&mut audio);
        }

        // Read the STATUS register (a NOP command returns it); drain every waiting packet
        loop {
            spi_job(&mut spi, &mut csn, &tx[..1], &mut rx[..1], &mut player, &mut audio);
            // RX_P_NO (STATUS bits 3:1) < 6 means the RX FIFO holds a payload
            if ((rx[0] >> 1) & 0x07) > 5 {
                break;
            }
            tx[0] = CMD_R_RX_PAYLOAD;
            spi_job(&mut spi, &mut csn, &tx, &mut rx, &mut player, &mut audio);
            tx[0] = CMD_NOP;
            player.push(&rx[1..]);
        }
    }
}
