//! Venue-compatible MessagePack hash for orders carrying a builder address.

use alloy::primitives::{keccak256, Address, B256};
use hypersdk::hypercore::types::{Action, OrderGrouping, OrderRequest};
use hypersdk::hypercore::{signing::agent_signing_hash, types::api::ActionRequest, Chain};
use serde::Serialize;

#[cfg(test)]
#[path = "signing_wire_test.rs"]
mod tests;

#[derive(Serialize)]
struct BuilderOrderWire<'a> {
    #[serde(rename = "type")]
    action_type: &'static str,
    orders: &'a [OrderRequest],
    grouping: &'a OrderGrouping,
    builder: BuilderWire,
}

/// Hyperliquid clears a scheduled cancel with `{"type":"scheduleCancel"}`
/// and hashes it without a `time` key. The SDK type always carries the key
/// (as `nil` when clearing), which recovers a different signer at the venue.
#[derive(Serialize)]
struct ClearScheduleCancelWire {
    #[serde(rename = "type")]
    action_type: &'static str,
}

#[derive(Serialize)]
struct BuilderWire {
    #[serde(rename = "b")]
    address: String,
    #[serde(rename = "f")]
    fee: u32,
}

/// The SDK's `Address` serializes as raw bytes in MessagePack. Hyperliquid
/// hashes the builder's lowercase `0x` text, as in its Python SDK. Keep the
/// signed bytes identical to the submitted action's JSON representation.
pub(crate) fn action_connection_id(
    action: &Action,
    nonce: u64,
    vault_address: Option<Address>,
    expires_after_ms: Option<u64>,
) -> Result<B256, rmp_serde::encode::Error> {
    if let Action::ScheduleCancel(schedule) = action {
        if schedule.time.is_none() {
            let wire = ClearScheduleCancelWire {
                action_type: "scheduleCancel",
            };
            return Ok(connection_id_from_action_bytes(
                rmp_serde::to_vec_named(&wire)?,
                nonce,
                vault_address,
                expires_after_ms,
            ));
        }
    }
    let Action::Order(batch) = action else {
        return action.hash(nonce, vault_address, expires_after_ms);
    };
    let Some(builder) = &batch.builder else {
        return action.hash(nonce, vault_address, expires_after_ms);
    };
    let wire = BuilderOrderWire {
        action_type: "order",
        orders: &batch.orders,
        grouping: &batch.grouping,
        builder: BuilderWire {
            address: format!("{:#x}", builder.builder_address),
            fee: builder.fee,
        },
    };
    Ok(connection_id_from_action_bytes(
        rmp_serde::to_vec_named(&wire)?,
        nonce,
        vault_address,
        expires_after_ms,
    ))
}

/// Append Hyperliquid's nonce, vault, and `expiresAfter` suffix to an
/// action's MessagePack bytes and hash the result.
fn connection_id_from_action_bytes(
    mut bytes: Vec<u8>,
    nonce: u64,
    vault_address: Option<Address>,
    expires_after_ms: Option<u64>,
) -> B256 {
    bytes.extend(nonce.to_be_bytes());
    if let Some(vault_address) = vault_address {
        bytes.push(1);
        bytes.extend(vault_address.as_slice());
    } else {
        bytes.push(0);
    }
    if let Some(expires_after_ms) = expires_after_ms {
        bytes.push(0);
        bytes.extend(expires_after_ms.to_be_bytes());
    }
    keccak256(bytes)
}

/// Recover the signer using the same builder text wire as live order signing.
/// The upstream SDK recovery remains correct for every action without a builder.
pub fn recover_action_request(request: &ActionRequest, chain: Chain) -> anyhow::Result<Address> {
    let custom_wire = matches!(&request.action, Action::Order(batch) if batch.builder.is_some())
        || matches!(&request.action, Action::ScheduleCancel(schedule) if schedule.time.is_none());
    if !custom_wire {
        return request.recover(chain);
    }
    let connection_id = action_connection_id(
        &request.action,
        request.nonce,
        request.vault_address,
        request.expires_after,
    )?;
    let prehash = agent_signing_hash(chain, connection_id);
    let recovery_id = match request.signature.v {
        27 => false,
        28 => true,
        _ => anyhow::bail!("invalid Hyperliquid signature recovery id"),
    };
    let signature =
        alloy::signers::Signature::new(request.signature.r, request.signature.s, recovery_id);
    Ok(signature.recover_address_from_prehash(&prehash)?)
}

/// Connection id of a TWAP action: the same MessagePack, nonce, vault, and
/// `expiresAfter` suffix scheme as every other agent-signed action.
pub(crate) fn twap_connection_id(
    twap: &crate::prepared_twap::TwapAction,
    nonce: u64,
    vault_address: Option<Address>,
    expires_after_ms: Option<u64>,
) -> Result<B256, rmp_serde::encode::Error> {
    Ok(connection_id_from_action_bytes(
        rmp_serde::to_vec_named(twap)?,
        nonce,
        vault_address,
        expires_after_ms,
    ))
}
