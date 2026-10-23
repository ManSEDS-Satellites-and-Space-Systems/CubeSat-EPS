# CubeSat-EPS

This is a EPS (Electric Power System) designed for 2U - 6U CubeSats.
The EPS contains 4 18650 Li-ion batteries in 2S2P configuartion.
Two power output rails are provided with 3.3v, 5v output, and 4A each.
Six MPPT (Maximum Power Point Tracking) boost converter connects the solar panels to the main bus.

## Hardware

Block diagram:

```mermaid
flowchart TD

    %% Solar Inputs
    SP1[Solar Panel 1] --> MPPT1[MPPT Boost 1]
    SP2[Solar Panel 2] --> MPPT2[MPPT Boost 2]
    SP3[Solar Panel 3] --> MPPT3[MPPT Boost 3]
    SP4[Solar Panel 4] --> MPPT4[MPPT Boost 4]
    SP5[Solar Panel 5] --> MPPT5[MPPT Boost 5]
    SP6[Solar Panel 6] --> MPPT6[MPPT Boost 6]

    MPPT1 --> BUS[Power Bus]
    MPPT2 --> BUS
    MPPT3 --> BUS
    MPPT4 --> BUS
    MPPT5 --> BUS
    MPPT6 --> BUS

    %% Battery
    BUS --> BATCTRL[Battery Charge / Protection]
    BATCTRL <--> BAT[Battery Pack]

    %% Monitoring
    BUS --> INA[INA3221 Current Monitor]
    BAT --> INA

    %% Power Rails
    BUS --> BUCK5[5V Buck Converter]
    BUS --> BUCK33[3.3V Buck Converter]

    BUCK5 --> V5[5V Rail]
    BUCK33 --> V33[3.3V Rail]

    %% MCU
    V33 --> MCU[STM32G431]
    INA --> MCU

    %% Communications
    MCU --> CAN[TCAN3414 CAN Transceiver]
    CAN --> CANBUS[CAN Bus]

    %% Watchdog
    MCU --> WDT[TPS3430 Watchdog]
    WDT -. Reset .-> MCU

    %% Loads
    V5 --> LOADS[5V Loads]
    V33 --> LOADS33[3.3V Loads]

    MCU --> H1[Heater 1 MOSFET]
    MCU --> H2[Heater 2 MOSFET]

    BAT --> H1
    BAT --> H2

    H1 --> HEATER1[Heater 1]
    H2 --> HEATER2[Heater 2]
```

Top view:

![front view](doc/front-view.png)

Back View:

![back view](doc/back-view.png)