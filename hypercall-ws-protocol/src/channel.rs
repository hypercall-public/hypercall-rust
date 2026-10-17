//! Channels a client can subscribe to on `/ws`.
//!
//! This is the single list of channel names. The API rejects a `Subscribe`
//! for any other name with an `Error` carrying [`UNKNOWN_CHANNEL_CODE`], and
//! the fanout applies the same check. Before the check existed the API
//! confirmed any name and then sent nothing, which left a client on a dead
//! subscription it could not detect.

/// `code` of the `Error` sent for a `Subscribe` naming an unknown channel.
pub const UNKNOWN_CHANNEL_CODE: &str = "unknown_channel";

/// Former channels. The API answers a subscribe to one of these with an
/// error that says where the data moved, rather than `unknown_channel`.
pub const RETIRED_CHANNELS: &[&str] = &["competition", "competition_engagement"];

/// A subscribable `/ws` channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WsChannel {
    Orderbook,
    Trades,
    MarketUpdates,
    OptionsChain,
    IndexPrices,
    /// One `IndicativeMarketData` frame per row.
    IndicativeMarketData,
    /// `IndicativeMarketDataBatch` frames: the rows of one short batch
    /// window per frame. Its `Subscribed` confirmation carries
    /// `"batch": true`.
    IndicativeMarketDataV2,
    OrderUpdates,
    Fills,
    Portfolio,
    Liquidation,
    Rfq,
    /// Private RFQ V2 change hints. Subscribing requires a signed read grant
    /// sent with `AuthenticateRfqV2` first.
    #[cfg(feature = "rfq-v2")]
    RfqV2,
    ExperimentalOrderUpdates,
    ExperimentalComboTrades,
    ExperimentalComboFills,
}

impl WsChannel {
    #[cfg(not(feature = "rfq-v2"))]
    pub const ALL: [Self; 15] = [
        Self::Orderbook,
        Self::Trades,
        Self::MarketUpdates,
        Self::OptionsChain,
        Self::IndexPrices,
        Self::IndicativeMarketData,
        Self::IndicativeMarketDataV2,
        Self::OrderUpdates,
        Self::Fills,
        Self::Portfolio,
        Self::Liquidation,
        Self::Rfq,
        Self::ExperimentalOrderUpdates,
        Self::ExperimentalComboTrades,
        Self::ExperimentalComboFills,
    ];

    #[cfg(feature = "rfq-v2")]
    pub const ALL: [Self; 16] = [
        Self::Orderbook,
        Self::Trades,
        Self::MarketUpdates,
        Self::OptionsChain,
        Self::IndexPrices,
        Self::IndicativeMarketData,
        Self::IndicativeMarketDataV2,
        Self::OrderUpdates,
        Self::Fills,
        Self::Portfolio,
        Self::Liquidation,
        Self::Rfq,
        Self::RfqV2,
        Self::ExperimentalOrderUpdates,
        Self::ExperimentalComboTrades,
        Self::ExperimentalComboFills,
    ];

    /// The exact wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Orderbook => "orderbook",
            Self::Trades => "trades",
            Self::MarketUpdates => "market_updates",
            Self::OptionsChain => "options_chain",
            Self::IndexPrices => "index_prices",
            Self::IndicativeMarketData => "indicative_market_data",
            Self::IndicativeMarketDataV2 => "indicative_market_data_v2",
            Self::OrderUpdates => "order_updates",
            Self::Fills => "fills",
            Self::Portfolio => "portfolio",
            Self::Liquidation => "liquidation",
            Self::Rfq => "rfq",
            #[cfg(feature = "rfq-v2")]
            Self::RfqV2 => "rfq_v2",
            Self::ExperimentalOrderUpdates => "experimental_order_updates",
            Self::ExperimentalComboTrades => "experimental_combo_trades",
            Self::ExperimentalComboFills => "experimental_combo_fills",
        }
    }

    /// Parse an exact wire name (case-sensitive, as the API has always
    /// matched channel names).
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|channel| channel.as_str() == name)
    }

    /// The message of the `Error` that rejects a subscribe to `name`.
    pub fn unknown_channel_message(name: &str) -> String {
        format!("Unknown channel '{name}'")
    }
}

impl std::fmt::Display for WsChannel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "channel_test.rs"]
mod tests;
