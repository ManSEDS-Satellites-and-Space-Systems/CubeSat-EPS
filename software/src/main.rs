#![no_std]
#![no_main]

mod mcp9800;
mod mppt;

use core::sync::atomic::{AtomicI32, Ordering};

use defmt_rtt as _;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_executor::Spawner;
use embassy_stm32::can::CanConfigurator;
use embassy_stm32::can::{RxBuf, TxBuf};
use embassy_stm32::gpio::{Level, Output, OutputType, Speed};
use embassy_stm32::i2c::{I2c, Master};
use embassy_stm32::mode::Async;
use embassy_stm32::time::Hertz;
use embassy_stm32::timer::low_level::CountingMode;
use embassy_stm32::timer::simple_pwm::{PwmPin, SimplePwm, SimplePwmChannel};
use embassy_stm32::{Config, bind_interrupts, can, dma, i2c, peripherals, timer};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Ticker};
// provides panic handler
use ina3221_dd::{
    AveragingMode, ChannelId, ConversionTime, INA3221_I2C_ADDR_GND, INA3221_I2C_ADDR_SCL,
    INA3221_I2C_ADDR_VS, Ina3221Async,
};
use panic_probe as _;
use static_cell::StaticCell;

use csp::CspListener;
use csp::drivers::embassy_stm32::CanStm32Driver;
use csp::interfaces::CanInterface;

use crate::mcp9800::Mcp9800;
use crate::mppt::Mppt;

bind_interrupts!(struct Irqs {
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
    I2C1_ER => i2c::ErrorInterruptHandler<peripherals::I2C1>;
    I2C1_EV => i2c::EventInterruptHandler<peripherals::I2C1>;
    I2C2_ER => i2c::ErrorInterruptHandler<peripherals::I2C2>;
    I2C2_EV => i2c::EventInterruptHandler<peripherals::I2C2>;
    DMA1_CHANNEL1 => dma::InterruptHandler<peripherals::DMA1_CH1>;
    DMA1_CHANNEL2 => dma::InterruptHandler<peripherals::DMA1_CH2>;
    DMA1_CHANNEL3 => dma::InterruptHandler<peripherals::DMA1_CH3>;
    DMA1_CHANNEL4 => dma::InterruptHandler<peripherals::DMA1_CH4>;
});

const SHUNT_RESISTANCE: f32 = 10.0;
const MCP9800_ADDR_A0: u8 = 0b1001000;
const MCP9800_ADDR_A5: u8 = 0b1001101;

/// Change to CV regulation when voltage reach
const BATTERY_CV_VOLTAGE: i32 = 8400;

static CAN_CSP_IFACE: StaticCell<CanInterface<CanStm32Driver>> = StaticCell::new();
static CAN_TX_BUF: StaticCell<TxBuf<4>> = StaticCell::new();
static CAN_RX_BUF: StaticCell<RxBuf<4>> = StaticCell::new();

/// Battery voltage in mV
static BAT_VOLTAGE: AtomicI32 = AtomicI32::new(0);
/// Positive bat current means discharging while negative means charging
static BAT_CURRENT: AtomicI32 = AtomicI32::new(0);
/// Solar panel output current in mA
static SOLAR_CURRENT: AtomicI32 = AtomicI32::new(0);
/// Total System current in mA
static SYS_CURRENT: AtomicI32 = AtomicI32::new(0);
/// temperature in milli-degrees celcius
static BAT_TEMP1: AtomicI32 = AtomicI32::new(0);
/// temperature in milli-degrees celcius
static BAT_TEMP2: AtomicI32 = AtomicI32::new(0);

/// 30 Hz 50% duty cycle to reset external windowed watchdog
#[embassy_executor::task]
async fn watchdog_reset(mut output: Output<'static>) {
    let mut ticker = Ticker::every(Duration::from_hz(60));

    loop {
        ticker.next().await;
        output.toggle();
    }
}

#[embassy_executor::task]
async fn bms_service() {
    let listener = CspListener::bind(10).await.expect("Failed to open port");

    loop {
        if let Ok(mut stream) = listener.accept(u32::MAX).await {
            stream.close().await;
        }
    }
}

#[embassy_executor::task]
async fn eps_service() {
    let listener = CspListener::bind(11).await.expect("Failed to open port");

    loop {
        if let Ok(mut stream) = listener.accept(u32::MAX).await {
            stream.close().await;
        }
    }
}

#[embassy_executor::task]
async fn mppt_control(
    dev1: I2cDevice<'static, NoopRawMutex, I2c<'static, Async, Master>>,
    dev2: I2cDevice<'static, NoopRawMutex, I2c<'static, Async, Master>>,
    mut pwm1: SimplePwmChannel<'static, peripherals::TIM2>,
    mut pwm2: SimplePwmChannel<'static, peripherals::TIM3>,
    mut pwm3: SimplePwmChannel<'static, peripherals::TIM3>,
    mut pwm4: SimplePwmChannel<'static, peripherals::TIM2>,
    mut pwm5: SimplePwmChannel<'static, peripherals::TIM4>,
    mut pwm6: SimplePwmChannel<'static, peripherals::TIM4>,
) {
    let mut mppt1 = Mppt::new(0.0);
    let mut mppt2 = Mppt::new(0.0);
    let mut mppt3 = Mppt::new(0.0);
    let mut mppt4 = Mppt::new(0.0);
    let mut mppt5 = Mppt::new(0.0);
    let mut mppt6 = Mppt::new(0.0);

    let mut ina_1 = Ina3221Async::new(dev1, INA3221_I2C_ADDR_GND);
    let mut ina_2 = Ina3221Async::new(dev2, INA3221_I2C_ADDR_VS);

    // This is a 50 Hz update loop, ADC samples every 204 us and averages 16 samples.
    // This gives us (0.204 + 0.204) x3 x 16 = 19.584 ms which is < 20 ms

    // 16 samples averaging
    let _ = ina_1.set_averaging_mode(AveragingMode::Samples16).await;
    let _ = ina_2.set_averaging_mode(AveragingMode::Samples16).await;

    // 588 us x2 conversion time
    let _ = ina_1.set_vbus_conversion_time(ConversionTime::Us204).await;
    let _ = ina_2.set_vbus_conversion_time(ConversionTime::Us204).await;
    let _ = ina_1
        .set_vshunt_conversion_time(ConversionTime::Us204)
        .await;
    let _ = ina_2
        .set_vshunt_conversion_time(ConversionTime::Us204)
        .await;

    let _ = ina_1.set_all_channels_enable(true, true, true);
    let _ = ina_2.set_all_channels_enable(true, true, true);

    let mut tick = Ticker::every(Duration::from_hz(50));

    // use closure so a channel failure will not affect others
    let update = async |ina: &mut Ina3221Async<_, _>, ch, mppt: &mut Mppt| {
        let Ok(mppt_v) = ina.get_bus_voltage_mv(ch).await else {
            return None;
        };
        let Ok(mppt_i) = ina.get_current_ma(ch, SHUNT_RESISTANCE).await else {
            return None;
        };

        return Some(mppt.update(mppt_v, mppt_i));
    };

    // infinite loop
    loop {
        // disable MPPT regulation when battery reaches CV voltage
        if BAT_VOLTAGE.load(Ordering::Relaxed) > BATTERY_CV_VOLTAGE {
            mppt1.reset();
            mppt2.reset();
            mppt3.reset();
            mppt4.reset();
            mppt5.reset();
            mppt6.reset();

            pwm1.set_duty_cycle(0);
            pwm2.set_duty_cycle(0);
            pwm3.set_duty_cycle(0);
            pwm4.set_duty_cycle(0);
            pwm5.set_duty_cycle(0);
            pwm6.set_duty_cycle(0);

            tick.next().await;
            continue;
        }
        if let Some(duty) = update(&mut ina_1, ChannelId::Channel1, &mut mppt1).await {
            pwm1.set_duty_cycle((pwm1.max_duty_cycle() as f32 * duty) as u32);
        }
        if let Some(duty) = update(&mut ina_1, ChannelId::Channel2, &mut mppt2).await {
            pwm2.set_duty_cycle((pwm2.max_duty_cycle() as f32 * duty) as u32);
        }
        if let Some(duty) = update(&mut ina_1, ChannelId::Channel3, &mut mppt3).await {
            pwm3.set_duty_cycle((pwm3.max_duty_cycle() as f32 * duty) as u32);
        }
        if let Some(duty) = update(&mut ina_2, ChannelId::Channel3, &mut mppt4).await {
            pwm4.set_duty_cycle((pwm4.max_duty_cycle() as f32 * duty) as u32);
        }
        if let Some(duty) = update(&mut ina_2, ChannelId::Channel2, &mut mppt5).await {
            pwm5.set_duty_cycle((pwm5.max_duty_cycle() as f32 * duty) as u32);
        }
        if let Some(duty) = update(&mut ina_2, ChannelId::Channel1, &mut mppt6).await {
            pwm6.set_duty_cycle((pwm6.max_duty_cycle() as f32 * duty) as u32);
        }

        tick.next().await;
    }
}

#[embassy_executor::task]
async fn bms_control(dev3: I2cDevice<'static, NoopRawMutex, I2c<'static, Async, Master>>) {
    let mut ina_3 = Ina3221Async::new(dev3, INA3221_I2C_ADDR_SCL);

    // This is a 100 Hz update loop, ADC samples every 332 us and averages 4 samples.
    // This gives us (0.204 + 0.588) x 3 x 4 = 9.504 ms which is < 10 ms

    // 8 samples averaging
    let _ = ina_3.set_averaging_mode(AveragingMode::Samples4).await;
    let _ = ina_3.set_vbus_conversion_time(ConversionTime::Us204).await;
    let _ = ina_3
        .set_vshunt_conversion_time(ConversionTime::Us588)
        .await;
    let _ = ina_3.set_all_channels_enable(true, true, true);

    let mut tick = Ticker::every(Duration::from_hz(100));

    loop {
        // read from ADC
        let v_bat = ina_3
            .get_bus_voltage_mv(ChannelId::Channel1)
            .await
            .unwrap_or(0.0);
        let i_bat = ina_3
            .get_current_ma(ChannelId::Channel1, SHUNT_RESISTANCE)
            .await
            .unwrap_or(0.0);
        let i_mppt = ina_3
            .get_current_ma(ChannelId::Channel2, SHUNT_RESISTANCE)
            .await
            .unwrap_or(0.0);
        let i_sys = ina_3
            .get_current_ma(ChannelId::Channel3, SHUNT_RESISTANCE)
            .await
            .unwrap_or(0.0);

        // store results
        BAT_VOLTAGE.store(v_bat as i32, Ordering::Relaxed);
        BAT_CURRENT.store(i_bat as i32, Ordering::Relaxed);
        SOLAR_CURRENT.store(i_mppt as i32, Ordering::Relaxed);
        SYS_CURRENT.store(i_sys as i32, Ordering::Relaxed);

        tick.next().await;
    }
}

#[embassy_executor::task]
async fn heater_control(
    dev4: I2cDevice<'static, NoopRawMutex, I2c<'static, Async, Master>>,
    dev5: I2cDevice<'static, NoopRawMutex, I2c<'static, Async, Master>>,
) {
    // MCP9800A0
    let mut temp1 = Mcp9800::new(dev4, MCP9800_ADDR_A0);
    // MCP9800A5
    let mut temp2 = Mcp9800::new(dev5, MCP9800_ADDR_A5);

    let mut tick = Ticker::every(Duration::from_hz(30));

    loop {
        let bat1_temp = temp1.temperature_celsius().await.unwrap_or(0.0);
        let bat2_temp = temp2.temperature_celsius().await.unwrap_or(0.0);

        BAT_TEMP1.store((bat1_temp * 1000.0) as i32, Ordering::Relaxed);
        BAT_TEMP2.store((bat2_temp * 1000.0) as i32, Ordering::Relaxed);

        // todo: PID heater control

        tick.next().await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = Config::default();
    {
        use embassy_stm32::rcc::*;
        // 8 Mhz external crystal oscillator
        config.rcc.hse = Some(Hse {
            freq: Hertz(8_000_000),
            mode: HseMode::Oscillator,
        });
        // setup PLL
        config.rcc.pll = Some(Pll {
            source: PllSource::HSE,    // 8 MHz
            prediv: PllPreDiv::DIV2,   // 4 MHz
            mul: PllMul::MUL72,        // 288 MHz
            divp: None,                // no ADC enabled
            divq: Some(PllQDiv::DIV6), // 48 MHz, Q clock (USB, I2S23, SAI1, FDCAN, QSPI)
            divr: Some(PllRDiv::DIV2), // Main system clock at 144 MHz
        });
        // use R clock as sysclk
        config.rcc.sys = Sysclk::PLL1_R;
        config.rcc.boost = true; // BOOST!
        config.rcc.mux.clk48sel = mux::Clk48sel::PLL1_Q;
    }

    let p = embassy_stm32::init(config);

    // external watchdog pin
    let extern_wdg = Output::new(p.PB5, Level::High, Speed::Medium);
    // spawn watchdog reset task
    spawner.spawn(watchdog_reset(extern_wdg).unwrap());

    // config i2c for ADC
    let mut i2c_config = i2c::Config::default();
    i2c_config.frequency = Hertz::khz(200);
    // init i2c master
    let i2c = I2c::new(
        p.I2C1, p.PA15, p.PB7, p.DMA1_CH1, p.DMA1_CH2, Irqs, i2c_config,
    );
    // shared I2C bus
    static I2C_BUS: StaticCell<Mutex<NoopRawMutex, I2c<'static, Async, Master>>> =
        StaticCell::new();
    // store static cell
    let i2c_bus = I2C_BUS.init(Mutex::new(i2c));

    // create shared devices on i2c bus
    let i2c_dev1 = I2cDevice::new(i2c_bus);
    let i2c_dev2 = I2cDevice::new(i2c_bus);
    let i2c_dev3 = I2cDevice::new(i2c_bus);
    let i2c_dev4 = I2cDevice::new(i2c_bus);
    let i2c_dev5 = I2cDevice::new(i2c_bus);

    let pwm1_pin = PwmPin::<'_, peripherals::TIM2, timer::Ch1>::new(p.PA5, OutputType::PushPull);
    let pwm2_pin = PwmPin::<'_, peripherals::TIM3, timer::Ch2>::new(p.PA7, OutputType::PushPull);
    let pwm3_pin = PwmPin::<'_, peripherals::TIM3, timer::Ch1>::new(p.PA6, OutputType::PushPull);
    let pwm4_pin = PwmPin::<'_, peripherals::TIM2, timer::Ch2>::new(p.PB3, OutputType::PushPull);
    let pwm5_pin = PwmPin::<'_, peripherals::TIM4, timer::Ch3>::new(p.PB8, OutputType::PushPull);
    let pwm6_pin = PwmPin::<'_, peripherals::TIM4, timer::Ch1>::new(p.PB6, OutputType::PushPull);

    let timer2_chs = SimplePwm::new(
        p.TIM2,
        Some(pwm1_pin),
        Some(pwm4_pin),
        None,
        None,
        Hertz::khz(500),
        CountingMode::default(),
    )
    .split();
    let timer3_chs = SimplePwm::new(
        p.TIM3,
        Some(pwm3_pin),
        Some(pwm2_pin),
        None,
        None,
        Hertz::khz(500),
        CountingMode::default(),
    )
    .split();
    let timer4_chs = SimplePwm::new(
        p.TIM4,
        Some(pwm6_pin),
        None,
        Some(pwm5_pin),
        None,
        Hertz::khz(500),
        CountingMode::default(),
    )
    .split();

    let pwm1 = timer2_chs.ch1;
    let pwm2 = timer3_chs.ch2;
    let pwm3 = timer3_chs.ch1;
    let pwm4 = timer2_chs.ch2;
    let pwm5 = timer4_chs.ch3;
    let pwm6 = timer4_chs.ch1;

    // spawn mppt control task
    spawner.spawn(mppt_control(i2c_dev1, i2c_dev2, pwm1, pwm2, pwm3, pwm4, pwm5, pwm6).unwrap());

    // spawn bms control task
    spawner.spawn(bms_control(i2c_dev3).unwrap());

    // spawn heater control task
    spawner.spawn(heater_control(i2c_dev4, i2c_dev5).unwrap());

    // initialise can
    let mut can_config = CanConfigurator::new(p.FDCAN1, p.PA11, p.PA12, Irqs);
    // set bitrate
    can_config.set_bitrate(500_000);

    can_config.properties().set_extended_filter(
        can::filter::ExtendedFilterSlot::_0,
        can::filter::ExtendedFilter::accept_all_into_fifo1(),
    );

    // start can
    let can = can_config
        .start(can::OperatingMode::NormalOperationMode)
        .buffered(CAN_TX_BUF.init(TxBuf::new()), CAN_RX_BUF.init(RxBuf::new()));

    let can_tx = can.writer();
    let can_rx = can.reader();
    // create driver
    let driver = CanStm32Driver::new(can_tx, can_rx, spawner.make_send());

    // add CSP interface
    csp::add_interface(CAN_CSP_IFACE.init(CanInterface::new(16, "UART", true, driver)));

    // spawn BMS service task
    spawner.spawn(bms_service().unwrap());
    // spawn EPS service task
    spawner.spawn(eps_service().unwrap());

    loop {
        csp::route_work().await;
    }
}
