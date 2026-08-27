#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tif {
    Alo,
    Gtc,
    Ioc,
}

impl std::fmt::Display for Tif {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Alo => write!(formatter, "ALO"),
            Self::Gtc => write!(formatter, "GTC"),
            Self::Ioc => write!(formatter, "IOC"),
        }
    }
}

#[cfg(feature = "venue")]
impl std::str::FromStr for Tif {
    type Err = hypercall_client::ClientError;

    fn from_str(value: &str) -> hypercall_client::error::Result<Self> {
        match value.to_lowercase().as_str() {
            "alo" => Ok(Self::Alo),
            "gtc" => Ok(Self::Gtc),
            "ioc" => Ok(Self::Ioc),
            _ => Err(hypercall_client::ClientError::InvalidInput(format!(
                "invalid tif '{value}', expected: alo, gtc, ioc"
            ))),
        }
    }
}
