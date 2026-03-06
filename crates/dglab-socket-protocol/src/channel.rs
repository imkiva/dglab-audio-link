use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DglabChannel {
    #[default]
    A,
    B,
}

impl DglabChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    pub const fn pulse_symbol(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }

    pub const fn strength_channel_id(self) -> u8 {
        match self {
            Self::A => 1,
            Self::B => 2,
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }
}
