# Radiophone (Elektroniikkapaja - Aalto University)

This repository contains the embedded firmware for a custom radiophone (two-way radio / walkie-talkie), developed as part of the **Elektroniikkapaja** (Electronics Workshop) course at **Aalto University**.

The firmware is written in bare-metal Rust utilizing the asynchronous [Embassy](https://embassy.dev/) framework.

---

## Project Overview

The objective of this project is to design and build a functional radiophone. The device handles audio input and output, digital signal processing/control, and radio-frequency (RF) transmission and reception.

### Hardware

- **Microcontroller**: STMicroelectronics STM32L432KC (ARM Cortex-M4 with FPU)
- **Development Board**: [NUCLEO-L432KC](https://www.st.com/en/evaluation-tools/nucleo-l432kc.html) (Nucleo-32 form factor)
- **Peripherals & RF Modules**:
  - NRF24L01 2.4 GHz RF transceiver module (SPI1)
  - MAX98357A I2S Class-D mono audio amplifier with passive speaker
  - INMP441 I2S digital MEMS microphone
  - Push-to-talk (PTT) push button
  - Power management

### Software Stack & Tools

- **Language**: [Rust](https://www.rust-lang.org/) (`#![no_std]`, bare-metal)
- **Target Architecture**: `thumbv7em-none-eabihf` (ARM Cortex-M4 with hard-float)
- **Async Runtime**: [Embassy](https://github.com/embassy-rs/embassy) (`embassy-stm32`, `embassy-executor`, `embassy-time`)
- **Logging & Diagnostics**: [`defmt`](https://github.com/knurling-rs/defmt) over RTT (`defmt-rtt`)
- **Flashing & Debugging**: [`probe-rs`](https://probe.rs/)
- **Linker & Stack Protection**: [`flip-link`](https://github.com/knurling-rs/flip-link)
- **Development Assistants**: Antigravity and Claude Code (agentic workflows for bringup, scaffolding, and verification)

---

## Pinout and Wiring Reference

### 1. NRF24L01 (2.4 GHz RF Transceiver – SPI1)
| NRF24L01 Pin | Nucleo-L432KC Pin / Connection | Description / Function |
| :--- | :--- | :--- |
| **VCC** | **3.3V** | Power supply (3.3V only! Do not connect to 5V) |
| **GND** | **GND** | Ground |
| **SCK** | **PA5** (A4) | SPI1 SCK (CN4 pin 8) |
| **MISO** | **PA6** (A5) | SPI1 MISO (CN4 pin 7) |
| **MOSI** | **PA7** (A6) | SPI1 MOSI (CN4 pin 6) |
| **CSN** | **PA11** (D10) | SPI Chip Select (CN3 pin 13) |
| **CE** | **PB0** (D3) | Chip Enable (CN3 pin 6) |

### 2. INMP441 (I2S MEMS Microphone)
| INMP441 Pin | Nucleo-L432KC Pin / Connection | Description / Function |
| :--- | :--- | :--- |
| **VDD** | **3.3V** | Power supply (3.3V) |
| **GND** | **GND** | Ground |
| **L/R** | **GND** (or 3.3V) | Selects Left Channel (GND) or Right Channel (3.3V) |
| **SCK** | **PA3** (A2) | I2S Bit Clock (connected in parallel with MAX98357A BCLK) |
| **WS** | **PA4** (A3) | I2S Word Select / LRC (connected in parallel with MAX98357A LRC) |
| **SD** | **PA1** (A1) | I2S Digital Audio Serial Data In to STM32 |

### 3. MAX98357A (I2S Class-D Amplifier) & Passive Speaker
| MAX98357A Pin | Nucleo-L432KC Pin / Connection | Description / Function |
| :--- | :--- | :--- |
| **VIN** | **5V** (or 3.3V) | Supply voltage (5V delivers higher output power) |
| **GND** | **GND** | Ground |
| **BCLK** | **PA3** (A2) | I2S Bit Clock (connected in parallel with INMP441 SCK) |
| **LRC** | **PA4** (A3) | I2S Word Select / Left-Right Clock (connected in parallel with INMP441 WS) |
| **DIN** | **PA10** (D0 / RX) | I2S Digital Audio Data Input to Amplifier |
| **SD_MODE** | **3.3V** (or unconnected) | Active mode (unconnected defaults to mono mix on breakout boards) |
| **GAIN** | *Leave unconnected* | Floating selects 9 dB standard gain |
| **"+"** | Speaker terminal 1 | Bridge-Tied Load (BTL) speaker output |
| **"-"** | Speaker terminal 2 | Bridge-Tied Load (BTL) speaker output |

### 4. Push-to-Talk (PTT) Button
| Button Terminal | Nucleo-L432KC Pin / Connection | Description / Function |
| :--- | :--- | :--- |
| **Terminal 1** | **PB1** (D6) | Button input, configured with the internal pull-up resistor |
| **Terminal 2** | **GND** | Ground |

The button is **active low**: PB1 reads high while released and low while pressed, so no external resistor is needed. Debouncing is done in firmware.

PB1 was chosen because it is a plain GPIO that is not used by anything else in the project: it avoids the NRF24L01 SPI1 pins (PA5/PA6/PA7), CSN/CE (PA11/PB0), the I2S audio pins (PA1/PA3/PA4/PA10), the LEDs (PB3/PB4), the ST-LINK virtual COM port (PA2/PA15) and the SWD debug pins (PA13/PA14). It also has its own EXTI line (EXTI1), so it can later be used as an interrupt-driven input without clashing with the NRF24 CE pin on PB0.

---

## Project Structure

```
├── .cargo/
│   └── config.toml      # Build target configuration, probe-rs runner, and linker flags
├── audio/               # Audio assets (8 kHz, 8-bit unsigned PCM) and conversion tool
│   ├── README.md        # Audio library documentation & selection guide
│   ├── audio_testi.raw  # User test speech audio file
│   ├── chime.raw        # Chime sound effect (Wikimedia Commons)
│   ├── bell.raw         # Bicycle bell ring (Wikimedia Commons)
│   ├── roger_beep.raw   # Walkie-talkie dual-tone roger beep
│   └── convert.py       # Helper script to convert standard WAV files to PCM
├── src/
│   ├── bin/
│   │   ├── blinky.rs        # Hardware bringup demo (blinks onboard LED LD3 on PB3)
│   │   ├── external_led.rs  # Blinks an external LED on D12 (PB4)
│   │   ├── nrf24_test.rs    # NRF24L01 SPI communication and register verification test
│   │   ├── nrf24_tx.rs      # NRF24L01 packet transmitter test (Enhanced ShockBurst TX)
│   │   ├── nrf24_rx.rs      # NRF24L01 packet receiver test (Enhanced ShockBurst RX)
│   │   ├── nrf24_audio_rx.rs # NRF24L01 audio receiver: plays received 8 kHz PCM packets on the speaker
│   │   ├── nrf24_audio_file_tx.rs # NRF24L01 audio transmitter: streams audio_testi.raw in a loop to nrf24_audio_rx
│   │   ├── nrf24_audio_tx.rs # NRF24L01 audio transmitter: streams live INMP441 microphone audio (8 kHz PCM) to nrf24_audio_rx
│   │   ├── speakers.rs      # MAX98357A I2S beep test (1 kHz sine tone on PA3/PA4/PA10)
│   │   ├── audio_player.rs  # MAX98357A I2S audio file player (PA3/PA4/PA10)
│   │   ├── mic_test.rs      # Microphone listen & speaker playback loop test (INMP441 + MAX98357A, auto-gain)
│   │   └── mic_direct.rs    # Real-time direct microphone-to-speaker pass-through (~62.5 µs latency, 64x gain)
│   ├── lib.rs           # Library root exporting reusable modules
│   ├── i2s_dma.rs       # Hardware-timed I2S output to the MAX98357A (TIM2 BCLK + DMA, same pins)
│   ├── i2s_mic_dma.rs   # Hardware-timed I2S clocks + INMP441 capture (TIM2 + DMA1 CH1/CH2)
│   ├── max98357a.rs     # MAX98357A & INMP441 I2S audio driver (cycle-accurate DWT timing)
│   ├── nrf24.rs         # NRF24L01 driver module (registers, TX/RX configuration, packet handling)
│   └── main.rs          # Main radiophone firmware application
├── Cargo.toml           # Project manifest and dependencies
└── README.md            # Project documentation
```

---

## Getting Started

### Prerequisites

1. **Rust Toolchain**: Install Rust via [rustup](https://rustup.rs/):
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **ARM Cortex-M4 Target**:
   ```bash
   rustup target add thumbv7em-none-eabihf
   ```

3. **Required Cargo Tools**:
   ```bash
   # Stack-overflow protection linker wrapper
   cargo install flip-link

   # Flashing and debugging tool
   cargo install probe-rs-tools
   ```

### Building and Running

Connect the Nucleo-L432KC board via USB.

1. **Run the Blinky Demo**:
   To verify that your toolchain, debug probe, and board connection work:
   ```bash
   cargo run --bin blinky
   ```
   This will flash the microcontroller, start execution, and stream defmt log output to your terminal. The onboard user LED (LD3, connected to `PB3`) will blink at 500 ms intervals.

2. **Run the External LED Demo**:
   Connect an LED with a series resistor (~330 Ω) between `D12` (`PB4`) and GND, then:
   ```bash
   cargo run --bin external_led
   ```
   This will flash and blink the external LED at 500 ms intervals.

3. **Run the NRF24L01 SPI Test**:
   ```bash
   cargo run --bin nrf24_test
   ```
   This binary reads internal registers (`CONFIG`, `STATUS`, `RF_CH`, `TX_ADDR`) and performs a register write/read-back verification to validate SPI communication over RTT defmt logs.

4. **Run the NRF24L01 Transmitter (TX)**:
   ```bash
   cargo run --bin nrf24_tx
   ```
   Configures the module on Channel 76 (2476 MHz) with 32-byte payloads and sends periodic ping packets. Supports Auto-Acknowledgment (Enhanced ShockBurst) and logs ACK receipt confirmation over RTT.

5. **Run the NRF24L01 Receiver (RX)**:
   ```bash
   cargo run --bin nrf24_rx
   ```
   Listens on Channel 76 (2476 MHz) for 32-byte packets, automatically sends hardware ACKs, logs payload text and raw hex bytes over RTT, and pulses the onboard LED on receipt.

6. **Run the MAX98357A I2S Speaker Beep Test**:
   ```bash
   cargo run --bin speakers
   ```
   Streams a pure 1000 Hz sine wave tone over I2S to the MAX98357A amplifier, with the onboard LED LD3 flashing in sync.

7. **Run the MAX98357A I2S Audio File Player**:
   ```bash
   cargo run --bin audio_player
   ```
   Plays real 8 kHz 8-bit PCM audio clips over I2S to the MAX98357A amplifier. By default, it plays `audio_testi.raw`. You can select which track to play (e.g. `audio_testi`, `chime`, `bell`, or `roger_beep`) or enable cycling through all tracks in [`src/bin/audio_player.rs`](file:///home/joona/elepaja/src/bin/audio_player.rs). See [`audio/README.md`](file:///home/joona/elepaja/audio/README.md) for details on adding custom audio files.

8. **Run the Microphone Listen & Speaker Echo Test**:
   ```bash
   cargo run --bin mic_test
   ```
   Listens to the INMP441 MEMS microphone for 2.0 seconds (16-bit PCM @ 8,000 Hz), removes DC bias, measures voice levels, normalises the volume with automatic digital gain (the INMP441 signal is very quiet), and immediately plays the captured voice back through the MAX98357A amplifier and speaker in a continuous echo loop.

9. **Run the Direct Live Microphone-to-Speaker Feed**:
   ```bash
   cargo run --bin mic_direct
   ```
   Streams live microphone audio directly to the speaker in real-time with near-zero latency (~62.5 µs, 1 audio frame), high-pass DC blocking, and 64x digital amplification (the raw INMP441 signal is under 1% of full scale; lower `GAIN` in `mic_direct.rs` if it feeds back).

10. **Run the NRF24L01 Audio Receiver**:
    ```bash
    cargo run --bin nrf24_audio_rx
    ```
    Listens on Channel 76 for audio packets and plays them on the MAX98357A speaker. Each 32-byte packet must carry 32 samples of 8-bit unsigned PCM (silence = 128) at 8 kHz (4 ms of audio), the same format as the `audio/*.raw` files. The speaker is driven by the hardware-timed I2S driver (`i2s_dma.rs`): TIM2 generates BCLK on PA3 and DMA writes LRC/DIN, so the clocks stay perfectly regular while the radio is read with ordinary blocking SPI (the bit-banged driver could not keep its clocks regular while talking to the radio, and the amplifier muted). Same wiring as the other audio programs. A 64 ms jitter buffer smooths packet timing; the log shows packets per second, buffer level, peak sample level, underruns, overruns and the lowest I2S queue level once per second (it should stay close to 63 frames). A beep at start-up confirms the speaker path. Set `DIAG` in the source to 1 (test tone, radio untouched) or 2 (test tone while the radio is polled) when debugging the speaker path. Compatible with `nrf24_audio_file_tx` from the transmitter project. Needs the radio wiring (SPI1) and the speaker wiring (PA3/PA4/PA10) at the same time; a transmitter sending audio packets is required to hear anything.

11. **Stream Live Microphone Audio over the Radio**:
    ```bash
    cargo run --bin nrf24_audio_tx
    ```
    Reads the INMP441 microphone (16 kHz, high-pass filtered, 64x gain, averaged down to 8 kHz) and sends it as 32-byte packets of 8-bit unsigned PCM on Channel 76. The mic is captured by hardware (`i2s_mic_dma.rs`: TIM2 generates BCLK/WS, DMA1 channels 1 and 2 drive WS and sample the mic data line), so radio SPI traffic never disturbs the audio clocks and no samples are lost. It transmits only while the push-to-talk button (PB1 to GND) is held, because an always-open mic near the receiver's speaker causes a rising feedback howl; set `REQUIRE_PTT = false` in the source to stream continuously. A noise gate (`GATE_OPEN` / `GATE_CLOSE` in the source) mutes the mic below speech level so faint speaker sound can't start a feedback loop; the log shows how much of the time the gate was open. Flash a second board with `nrf24_audio_rx` to hear it on the speaker. Wiring is the same as above (the MAX98357A does not need to be connected on the transmitter). RTT logs show ACK counts, mic peak level and capture overruns (should stay 0).

12. **Stream an Audio File over the Radio**:
    ```bash
    cargo run --bin nrf24_audio_file_tx
    ```
    Sends `audio/audio_testi.raw` (8 kHz, 8-bit unsigned PCM) as 32-byte packets every 4 ms on Channel 76, repeating with a 2 s pause. Useful for testing `nrf24_audio_rx` without a microphone.

13. **Run the Main Firmware**:
    ```bash
    cargo run
    ```
    For optimized release builds:
    ```bash
    cargo run --release
    ```

---

## Roadmap

- [x] Hardware setup and project template with Embassy on STM32L432KC
- [x] Basic onboard LED blinking and defmt logging over RTT
- [x] External LED output demo (GPIO PB4)
- [x] NRF24L01 SPI communication test and register verification
- [x] NRF24L01 packet TX/RX driver and test programs (`nrf24_tx`, `nrf24_rx`)
- [x] MAX98357A I2S amplifier audio output driver (`speakers` beep test & `audio_player`)
- [x] INMP441 I2S MEMS microphone capture and echo test (`mic_test`)
- [x] Real-time direct microphone-to-speaker live feed (`mic_direct`)
- [x] NRF24L01 audio packet receiver playing on the speaker (`nrf24_audio_rx`)
- [x] PTT button pin selected and documented (PB1, active low)
- [ ] Push-to-Talk (PTT) button input handling with debouncing
- [ ] Half-duplex transmit and receive state machine
- [ ] Enclosure and final hardware assembly

---

## License

This project is created for educational purposes in the Elektroniikkapaja course at Aalto University.
