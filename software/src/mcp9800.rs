use embedded_hal_async::i2c::I2c;

pub const REG_TEMP: u8 = 0x00;
pub const REG_CONFIG: u8 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resolution {
    /// 30 ms convertion time
    #[default]
    Bits9,
    Bits10,
    Bits11,
    /// 240 ms convertion time
    Bits12,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FaultQueue {
    #[default]
    L1,
    L2,
    L4,
    L6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Polarity {
    ActiveHigh,
    #[default]
    ActiveLow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    Interrupt,
    #[default]
    Comparator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Config {
    pub one_shot: bool,
    pub resolution: Resolution,
    pub fault_queue: FaultQueue,
    pub polarity: Polarity,
    pub mode: Mode,
    pub shutdown: bool,
}

pub struct Mcp9800<I2C> {
    i2c: I2C,
    address: u8,
}

impl<I2C> Mcp9800<I2C> {
    pub const fn new(i2c: I2C, address: u8) -> Self {
        Self { i2c, address }
    }
}

#[allow(unused)]
impl<I2C, E> Mcp9800<I2C>
where
    I2C: I2c<Error = E>,
{
    /// Read raw temperature register.
    async fn read_temp_raw(&mut self) -> Result<u16, E> {
        let mut buf = [0u8; 2];

        self.i2c
            .write_read(self.address, &[REG_TEMP], &mut buf)
            .await?;

        Ok(u16::from_be_bytes(buf))
    }

    /// Read temperature in °C.
    pub async fn temperature_celsius(&mut self) -> Result<f32, E> {
        let raw = self.read_temp_raw().await?;

        // Upper 12 bits contain a signed fixed-point value
        let value = (raw >> 4) as i16;

        // Sign-extend 12-bit two's complement
        let signed = if (value & 0x0800) != 0 {
            value | !0x0FFF
        } else {
            value
        };

        Ok(signed as f32 * 0.0625)
    }

    /// Read configuration register.
    pub async fn config(&mut self) -> Result<Config, E> {
        let mut buf = [0u8; 1];

        self.i2c
            .write_read(self.address, &[REG_CONFIG], &mut buf)
            .await?;

        let v = buf[0];

        Ok(Config {
            one_shot: (v & 0b10000000) != 0,
            resolution: match v & 0b01100000 {
                0b01100000 => Resolution::Bits12,
                0b01000000 => Resolution::Bits11,
                0b00100000 => Resolution::Bits10,
                0b00000000 => Resolution::Bits9,
                _ => unreachable!(),
            },
            fault_queue: match v & 0b00011000 {
                0b00011000 => FaultQueue::L6,
                0b00010000 => FaultQueue::L4,
                0b00001000 => FaultQueue::L2,
                0b00000000 => FaultQueue::L1,
                _ => unreachable!(),
            },
            polarity: if (v & 0b00000100) != 0 {
                Polarity::ActiveHigh
            } else {
                Polarity::ActiveLow
            },
            mode: if (v & 0b00000010) != 0 {
                Mode::Interrupt
            } else {
                Mode::Comparator
            },
            shutdown: (v & 0b00000001) != 0,
        })
    }

    /// Write configuration register.
    pub async fn set_config(&mut self, cfg: Config) -> Result<(), E> {
        let mut v = 0u8;

        if cfg.one_shot {
            v |= 0b1000_0000;
        }

        v |= match cfg.resolution {
            Resolution::Bits12 => 0b0110_0000,
            Resolution::Bits11 => 0b0100_0000,
            Resolution::Bits10 => 0b0010_0000,
            Resolution::Bits9 => 0b0000_0000,
        };

        v |= match cfg.fault_queue {
            FaultQueue::L6 => 0b0001_1000,
            FaultQueue::L4 => 0b0001_0000,
            FaultQueue::L2 => 0b0000_1000,
            FaultQueue::L1 => 0b0000_0000,
        };

        if matches!(cfg.polarity, Polarity::ActiveHigh) {
            v |= 0b0000_0100;
        }

        if matches!(cfg.mode, Mode::Interrupt) {
            v |= 0b0000_0010;
        }

        if cfg.shutdown {
            v |= 0b0000_0001;
        }

        self.i2c.write(self.address, &[REG_CONFIG, v]).await
    }
}
