use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral as PlatformPeripheral};

pub enum BthrSignal {
    HeartRate {
        heart_rate: u8,
    },
    DiscoveredPeripherals (Vec<String>),
    StartScan,
}