use btleplug::platform::Peripheral;



pub enum BthrSignal {
    HeartRate {
        heart_rate: u8,
    },
    DiscoveredPeripherals(Vec<String>),
    ScanStarted,
    ScanStopped,
}

pub enum GuiSignal {
    StartScanning,
    StopScanning,
    ConnectDevice(String),
}

pub enum TaskSignal {
    PeripheralsFound(Vec<Peripheral>),
    NotificationStreamAcquired,
    HeartRatePing,
    PeripheralNotFound(String),
    ConnectionFailed,
    DiscoveringServicesFailed,
    HrCharNotFound,
    CharSubscriptionFailed,
    NotificationStreamFailed,
    PeripheralDisconnected,
}