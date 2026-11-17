use super::*;
use alloy::primitives::Signature;
use std::str::FromStr;

const PRIVATE_KEY: &str = "0x0000000000000000000000000000000000000000000000000000000000000001";
const ACCOUNT: &str = "0x00000000000000000000000000000000000000a1";
const NONCE: u64 = 1_785_672_000_000;

fn payload(tif: PerpTimeInForce) -> TeeSpotOrderSignature {
    TeeSpotOrderSignature {
        account: ACCOUNT.parse().unwrap(),
        nonce: NONCE,
        action: TeeSpotOrderAction {
            asset: 10_107,
            is_buy: true,
            limit_px: 3_000_000_000,
            sz: 200_000_000,
            tif,
            cloid: 42,
        },
    }
}

#[test]
fn the_typed_data_matches_the_enclave() {
    // The enclave and API verify exactly these type strings
    // (hypercall-account-action-intent `spot_order`).
    assert_eq!(
        HCSpotOrder::eip712_encode_type(),
        "HCSpotOrder(address account,uint64 nonce,SpotOrder action)SpotOrder(uint32 asset,bool isBuy,uint64 limitPx,uint64 sz,uint8 encodedTif,uint128 cloid)"
    );
    assert_eq!(
        HCSpotCancel::eip712_encode_type(),
        "HCSpotCancel(address account,uint64 nonce,SpotCancel action)SpotCancel(uint32 asset,uint128 cloid)"
    );
}

#[tokio::test]
async fn the_wallet_signs_the_order_digest() {
    let wallet = HypercallWallet::from_private_key(PRIVATE_KEY, 999).unwrap();
    let signature = wallet
        .sign_tee_spot_order_payload(payload(PerpTimeInForce::Gtc))
        .await
        .unwrap();
    let digest = tee_spot_order_signing_hash(999, &payload(PerpTimeInForce::Gtc)).unwrap();
    let recovered = Signature::from_str(&signature)
        .unwrap()
        .recover_address_from_prehash(&digest)
        .unwrap();
    assert_eq!(recovered, wallet.address().as_wallet_address().inner());
}

#[test]
fn alo_is_refused() {
    assert!(tee_spot_order_signing_hash(999, &payload(PerpTimeInForce::Alo)).is_err());
}
