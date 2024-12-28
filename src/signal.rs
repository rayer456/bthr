use btleplug::platform::Peripheral;



pub enum BthrSignal {
    HeartRate {
        heart_rate: u8,
    },
    DiscoveredPeripherals(Vec<String>),
    StartScan,
}

pub enum GuiSignal {
    StartScanning,
    StopScanning,
    ConnectDevice(String),
    Empty,
}

pub enum ScanSignal {
    Peripherals(Vec<Peripheral>)
}