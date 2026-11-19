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
//! The I2S clocks are bit-banged by the CPU and must be perfectly regular: even a 1 us
//! stretch of one BCLK pulse between frames makes the MAX98357A buzz or go silent. So in
//! steady state *no work at all happens between frames*. Everything runs inside the frame,
//! in the idle time after BCLK rising edges, through `exchange_frame_hook`:
//!   * bit 17: pick the next audio sample and update the playback state,
//!   * every 8th bit: advance a register-level SPI state machine by one byte that polls the
//!     radio and writes received payload bytes straight into the jitter buffer.
//! A jitter buffer absorbs packet timing variation: playback starts once `PREBUFFER`
//! samples have arrived and re-buffers after an underrun. Statistics are logged once per
//! second (a short click in the audio).

use defmt::{error, info};
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::spi::{Config, Spi};
use embassy_stm32::time::Hertz;
use nucleo_l432kc_app::max98357a::{config_80mhz, Max98357a};
use nucleo_l432kc_app::nrf24::{Nrf24, PAYLOAD_SIZE};
use {defmt_rtt as _, panic_probe as _};

/// Jitter buffer capacity in samples (must be a power of two): 2048 = 256 ms.
const BUF_SIZE: usize = 2048;
const MASK: usize = BUF_SIZE - 1;
/// Samples to collect before playback (re)starts: 512 = 64 ms.
const PREBUFFER: usize = 512;
/// Playback volume shift (0 = unchanged, 1 = 2x, ...). Samples are clipped.
const VOLUME_SHIFT: u32 = 0;
/// Diagnostic mode: 0 = play received audio, 1 = play a 1 kHz tone instead while "playing",
/// 2 = play the tone continuously while the radio is still polled,
/// 3 = play the tone continuously and never touch the radio SPI.
const DIAG: u8 = 0;
/// One period of a 1 kHz tone at 8 kHz.
const TONE_8K: [i16; 8] = [0, 21213, 30000, 21213, 0, -21213, -30000, -21213];

/// Idle SPI steps (one step per 8 BCLK bits) between STATUS polls when no packet is waiting:
/// 64 steps = 8 frames = 0.5 ms, well below the 4 ms packet interval.
const POLL_WAIT_STEPS: u32 = 16;

const SPI1_BASE: usize = 0x4001_3000;
const SPI_CR2: *mut u32 = (SPI1_BASE + 0x04) as *mut u32;
const SPI_DR8: *mut u8 = (SPI1_BASE + 0x0C) as *mut u8;
const CR2_FRXTH: u32 = 1 << 12;
const GPIOA_BSRR: *mut u32 = 0x4800_0018 as *mut u32;
const GPIOB_BSRR: *mut u32 = 0x4800_0418 as *mut u32;
const CSN_LOW: u32 = 1 << (11 + 16); // PA11
const CSN_HIGH: u32 = 1 << 11;
const LED_ON: u32 = 1 << 3; // PB3
const LED_OFF: u32 = 1 << (3 + 16);
const CMD_NOP: u8 = 0xFF;
const CMD_R_RX_PAYLOAD: u8 = 0x61;

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    /// Idle: waiting between status polls
    Idle,
    /// STATUS poll command sent, waiting to read result
    StatusWait,
    /// R_RX_PAYLOAD command sent, waiting to read command byte response
    ReadCmdWait,
    /// Reading 32 payload data bytes (1 byte per invocation)
    ReadPayload,
}

/// Receiver state: jitter buffer, playback feeder and non-blocking SPI state machine.
struct Rx {
    // Jitter buffer of raw unsigned 8-bit samples
    data: [u8; BUF_SIZE],
    head: usize,
    count: usize,
    // Playback
    playing: bool,
    sample: i16,
    odd: bool,
    tx: u16,
    tone_phase: usize,
    // SPI state machine
    stage: Stage,
    recvd: usize,
    wait: u32,
    wbase: usize,
    // Statistics
    peak: u16,
    packets: u32,
    underruns: u32,
    overruns: u32,
}

impl Rx {
    /// Hook for bit 17: choose the sample for the next frame (a new one every second frame).
    #[inline(always)]
    fn next_sample(&mut self) {
        self.odd = !self.odd;
        if self.odd {
            return;
        }
        if !self.playing && self.count >= PREBUFFER {
            self.playing = true;
            unsafe { core::ptr::write_volatile(GPIOB_BSRR, LED_ON) };
        } else if self.playing && self.count == 0 {
            self.playing = false;
            self.underruns += 1;
            unsafe { core::ptr::write_volatile(GPIOB_BSRR, LED_OFF) };
        }
        if DIAG >= 2 {
            self.tone_phase += 1;
            self.sample = TONE_8K[self.tone_phase & 7];
        } else if self.playing {
            let raw = self.data[self.head];
            self.head = (self.head + 1) & MASK;
            self.count -= 1;
            self.sample = if DIAG == 1 {
                self.tone_phase += 1;
                TONE_8K[self.tone_phase & 7]
            } else {
                let v = ((raw as i32) - 128) << (8 + VOLUME_SHIFT);
                v.clamp(-32768, 32767) as i16
            };
            let a = self.sample.unsigned_abs();
            if a > self.peak {
                self.peak = a;
            }
        } else {
            self.sample = 0;
        }
        self.tx = self.sample as u16;
    }

    /// Single non-blocking SPI step. Runs at bits 33 and 49 (~15.6 us apart).
    /// At 4 MHz SPI, an 8-bit byte takes 2.0 us, so the previous byte is 100% finished shifting.
    /// Execution time is ~12-16 cycles, well within the 39-cycle BCLK half-period.
    #[inline(always)]
    fn spi_step(&mut self) {
        match self.stage {
            Stage::Idle => {
                if self.wait > 0 {
                    self.wait -= 1;
                } else {
                    unsafe { core::ptr::write_volatile(GPIOA_BSRR, CSN_LOW) };
                    unsafe { core::ptr::write_volatile(SPI_DR8, CMD_NOP) };
                    self.stage = Stage::StatusWait;
                }
            }
            Stage::StatusWait => {
                let status = unsafe { core::ptr::read_volatile(SPI_DR8) };
                unsafe { core::ptr::write_volatile(GPIOA_BSRR, CSN_HIGH) };
                if ((status >> 1) & 0x07) <= 5 {
                    if self.count + PAYLOAD_SIZE > BUF_SIZE {
                        self.head = (self.head + PAYLOAD_SIZE) & MASK;
                        self.count -= PAYLOAD_SIZE;
                        self.overruns += 1;
                    }
                    self.wbase = (self.head + self.count) & MASK;
                    self.recvd = 0;
                    unsafe { core::ptr::write_volatile(GPIOA_BSRR, CSN_LOW) };
                    unsafe { core::ptr::write_volatile(SPI_DR8, CMD_R_RX_PAYLOAD) };
                    self.stage = Stage::ReadCmdWait;
                } else {
                    self.wait = POLL_WAIT_STEPS;
                    self.stage = Stage::Idle;
                }
            }
            Stage::ReadCmdWait => {
                let _ = unsafe { core::ptr::read_volatile(SPI_DR8) };
                unsafe { core::ptr::write_volatile(SPI_DR8, CMD_NOP) };
                self.stage = Stage::ReadPayload;
            }
            Stage::ReadPayload => {
                let b = unsafe { core::ptr::read_volatile(SPI_DR8) };
                self.data[(self.wbase + self.recvd) & MASK] = b;
                self.recvd += 1;
                if self.recvd < PAYLOAD_SIZE {
                    unsafe { core::ptr::write_volatile(SPI_DR8, CMD_NOP) };
                } else {
                    unsafe { core::ptr::write_volatile(GPIOA_BSRR, CSN_HIGH) };
                    self.count += PAYLOAD_SIZE;
                    self.packets += 1;
                    self.stage = Stage::Idle;
                }
            }
        }
    }

    /// Clock one I2S frame; all per-frame work runs inside it.
    #[inline(always)]
    fn frame(&mut self, audio: &mut Max98357a) {
        let tx = self.tx;
        audio.exchange_frame_hook(tx, |bit| {
            if bit == 17 {
                self.next_sample();
            } else if DIAG != 3 {
                self.spi_step();
            }
        });
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // 80 MHz SYSCLK via PLL for the cycle-accurate I2S timing
    let p = embassy_stm32::init(config_80mhz());

    // Onboard LED LD3 (PB3): lit while audio is being played (set from the frame hook)
    let mut led = Output::new(p.PB3, Level::Low, Speed::Low);
    let ce = Output::new(p.PB0, Level::Low, Speed::Low);
    let csn = Output::new(p.PA11, Level::High, Speed::High);

    info!("=======================================================");
    info!("=== NRF24L01 Audio Receiver -> MAX98357A Speaker    ===");
    info!("=======================================================");
    info!("Expecting 32-byte packets of 8-bit unsigned PCM @ 8 kHz on channel 76.");

    let mut spi_config = Config::default();
    spi_config.frequency = Hertz(4_000_000);
    // Slow edges: less noise coupled into the neighbouring I2S wires
    spi_config.rise_fall_speed = Speed::Low;
    let spi = Spi::new_blocking(p.SPI1, p.PA5, p.PA7, p.PA6, spi_config);
    let mut radio = Nrf24::new(spi, ce, csn);

    if !radio.init().await {
        error!("Failed to initialize NRF24L01! Check wiring, 3.3V power, and GND.");
        loop {
            led.toggle();
            embassy_time::Timer::after_millis(100).await;
        }
    }
    radio.config_rx().await;
    info!("Radio listening on channel 76.");

    let mut audio = Max98357a::new(p.PA3, p.PA4, p.PA10);

    // Startup beep: confirms the speaker path works before any radio audio arrives
    audio.play_tone(1000, 150);

    // From here on SPI1 is driven by the state machine (`radio` keeps the peripheral alive).
    unsafe { core::ptr::write_volatile(SPI_CR2, core::ptr::read_volatile(SPI_CR2) | CR2_FRXTH) };
    let mut rx = Rx {
        data: [128; BUF_SIZE],
        head: 0,
        count: 0,
        playing: false,
        sample: 0,
        odd: true,
        tx: 0,
        tone_phase: 0,
        stage: Stage::Idle,
        recvd: 0,
        wait: 0,
        wbase: 0,
        peak: 0,
        packets: 0,
        underruns: 0,
        overruns: 0,
    };

    loop {
        // 16,000 frames = 1 second. This loop adds only a counter compare between frames.
        for _ in 0..16_000 {
            rx.frame(&mut audio);
        }
        info!(
            "Packets: {}, buffered: {} samples, peak: {}, underruns: {}, overruns: {}, late bits: {}",
            rx.packets,
            rx.count,
            rx.peak,
            rx.underruns,
            rx.overruns,
            audio.late_bits()
        );
        rx.packets = 0;
        rx.peak = 0;
    }
}
