/// The machine the pipeline runs on. One adapter per platform.
pub trait Machine: Send + Sync {
    /// Power source, Low Power Mode, and thermal state. Never prompts.
    fn conditions(&self) -> Conditions;
    /// Applies to the calling thread only. Engines also get the pace's thread count.
    fn set_pace(&self, pace: Pace);
}

/// How hard the pipeline works.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pace {
    Fast,
    /// Fewer threads at background priority, which macOS keeps on the efficiency cores.
    Light,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Power {
    Ac,
    Battery,
    /// No power source could be read, as on a desktop with no adapter for this platform.
    #[default]
    Unknown,
}

/// The system's thermal state, as macOS reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Thermal {
    #[default]
    Nominal,
    Fair,
    Serious,
    Critical,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Conditions {
    pub power: Power,
    pub low_power: bool,
    pub thermal: Thermal,
}
