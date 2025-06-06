//! Cryptographic algorithms

/// Opaque error type for sender recovery.
#[derive(Debug, Default, thiserror::Error)]
#[error("Failed to recover the signer")]
pub struct RecoveryError;

use alloy_primitives::U256;

#[cfg(any(feature = "secp256k1", feature = "k256"))]
use alloy_primitives::Signature;

/// The order of the secp256k1 curve, divided by two. Signatures that should be checked according
/// to EIP-2 should have an S value less than or equal to this.
///
/// `57896044618658097711785492504343953926418782139537452191302581570759080747168`
pub const SECP256K1N_HALF: U256 = U256::from_be_bytes([
    0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0x5D, 0x57, 0x6E, 0x73, 0x57, 0xA4, 0x50, 0x1D, 0xDF, 0xE9, 0x2F, 0x46, 0x68, 0x1B, 0x20, 0xA0,
]);

/// Secp256k1 cryptographic functions.
#[cfg(any(feature = "secp256k1", feature = "k256"))]
pub mod secp256k1 {
    pub use imp::{public_key_to_address, sign_message};

    use super::*;
    use alloy_primitives::{Address, B256};

    #[cfg(not(feature = "secp256k1"))]
    use super::impl_k256 as imp;
    #[cfg(feature = "secp256k1")]
    use super::impl_secp256k1 as imp;

    /// Recover signer from message hash, _without ensuring that the signature has a low `s`
    /// value_.
    ///
    /// Using this for signature validation will succeed, even if the signature is malleable or not
    /// compliant with EIP-2. This is provided for compatibility with old signatures which have
    /// large `s` values.
    pub fn recover_signer_unchecked(
        signature: &Signature,
        hash: B256,
    ) -> Result<Address, RecoveryError> {
        let mut sig: [u8; 65] = [0; 65];

        sig[0..32].copy_from_slice(&signature.r().to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&signature.s().to_be_bytes::<32>());
        sig[64] = signature.v() as u8;

        // NOTE: we are removing error from underlying crypto library as it will restrain primitive
        // errors and we care only if recovery is passing or not.
        imp::recover_signer_unchecked(&sig, &hash.0).map_err(|_| RecoveryError)
    }

    /// Recover signer address from message hash. This ensures that the signature S value is
    /// greater than `secp256k1n / 2`, as specified in
    /// [EIP-2](https://eips.ethereum.org/EIPS/eip-2).
    ///
    /// If the S value is too large, then this will return `None`
    pub fn recover_signer(signature: &Signature, hash: B256) -> Result<Address, RecoveryError> {
        if signature.s() > SECP256K1N_HALF {
            return Err(RecoveryError);
        }
        recover_signer_unchecked(signature, hash)
    }
}

#[cfg(any(test, feature = "secp256k1"))]
mod impl_secp256k1 {
    pub(crate) use ::secp256k1::Error;
    use ::secp256k1::{
        ecdsa::{RecoverableSignature, RecoveryId},
        Message, PublicKey, SecretKey, SECP256K1,
    };
    use alloy_primitives::{keccak256, Address, Signature, B256, U256};

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    pub(crate) fn recover_signer_unchecked(
        sig: &[u8; 65],
        msg: &[u8; 32],
    ) -> Result<Address, Error> {
        let sig =
            RecoverableSignature::from_compact(&sig[0..64], RecoveryId::try_from(sig[64] as i32)?)?;

        let public = SECP256K1.recover_ecdsa(&Message::from_digest(*msg), &sig)?;
        Ok(public_key_to_address(public))
    }

    /// Signs message with the given secret key.
    /// Returns the corresponding signature.
    pub fn sign_message(secret: B256, message: B256) -> Result<Signature, Error> {
        let sec = SecretKey::from_slice(secret.as_ref())?;
        let s = SECP256K1.sign_ecdsa_recoverable(&Message::from_digest(message.0), &sec);
        let (rec_id, data) = s.serialize_compact();

        let signature = Signature::new(
            U256::try_from_be_slice(&data[..32]).expect("The slice has at most 32 bytes"),
            U256::try_from_be_slice(&data[32..64]).expect("The slice has at most 32 bytes"),
            i32::from(rec_id) != 0,
        );
        Ok(signature)
    }

    /// Converts a public key into an ethereum address by hashing the encoded public key with
    /// keccak256.
    pub fn public_key_to_address(public: PublicKey) -> Address {
        // strip out the first byte because that should be the SECP256K1_TAG_PUBKEY_UNCOMPRESSED
        // tag returned by libsecp's uncompressed pubkey serialization
        let hash = keccak256(&public.serialize_uncompressed()[1..]);
        Address::from_slice(&hash[12..])
    }
}

#[cfg(feature = "k256")]
#[cfg_attr(feature = "secp256k1", allow(unused, unreachable_pub))]
mod impl_k256 {
    pub(crate) use k256::ecdsa::Error;

    use super::*;
    use alloy_primitives::{keccak256, Address, B256};
    use k256::ecdsa::{RecoveryId, SigningKey, VerifyingKey};

    /// Recovers the address of the sender using secp256k1 pubkey recovery.
    ///
    /// Converts the public key into an ethereum address by hashing the public key with keccak256.
    ///
    /// This does not ensure that the `s` value in the signature is low, and _just_ wraps the
    /// underlying secp256k1 library.
    pub(crate) fn recover_signer_unchecked(
        sig: &[u8; 65],
        msg: &[u8; 32],
    ) -> Result<Address, Error> {
        let mut signature = k256::ecdsa::Signature::from_slice(&sig[0..64])?;
        let mut recid = sig[64];

        // normalize signature and flip recovery id if needed.
        if let Some(sig_normalized) = signature.normalize_s() {
            signature = sig_normalized;
            recid ^= 1;
        }
        let recid = RecoveryId::from_byte(recid).expect("recovery ID is valid");

        // recover key
        let recovered_key = VerifyingKey::recover_from_prehash(&msg[..], &signature, recid)?;
        Ok(public_key_to_address(recovered_key))
    }

    /// Signs message with the given secret key.
    /// Returns the corresponding signature.
    pub fn sign_message(secret: B256, message: B256) -> Result<Signature, Error> {
        let sec = SigningKey::from_slice(secret.as_ref())?;
        sec.sign_prehash_recoverable(&message.0).map(Into::into)
    }

    /// Converts a public key into an ethereum address by hashing the encoded public key with
    /// keccak256.
    pub fn public_key_to_address(public: VerifyingKey) -> Address {
        let hash = keccak256(&public.to_encoded_point(/* compress = */ false).as_bytes()[1..]);
        Address::from_slice(&hash[12..])
    }
}

#[cfg(test)]
mod tests {

    #[cfg(feature = "secp256k1")]
    #[test]
    fn sanity_ecrecover_call_secp256k1() {
        use super::impl_secp256k1::*;
        use alloy_primitives::B256;

        let (secret, public) = secp256k1::generate_keypair(&mut rand::thread_rng());
        let signer = public_key_to_address(public);

        let message = b"hello world";
        let hash = alloy_primitives::keccak256(message);
        let signature =
            sign_message(B256::from_slice(&secret.secret_bytes()[..]), hash).expect("sign message");

        let mut sig: [u8; 65] = [0; 65];
        sig[0..32].copy_from_slice(&signature.r().to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&signature.s().to_be_bytes::<32>());
        sig[64] = signature.v() as u8;

        assert_eq!(recover_signer_unchecked(&sig, &hash), Ok(signer));
    }

    #[cfg(feature = "k256")]
    #[test]
    fn sanity_ecrecover_call_k256() {
        use super::impl_k256::*;
        use alloy_primitives::B256;

        let secret = k256::ecdsa::SigningKey::random(&mut rand::thread_rng());
        let public = *secret.verifying_key();
        let signer = public_key_to_address(public);

        let message = b"hello world";
        let hash = alloy_primitives::keccak256(message);
        let signature =
            sign_message(B256::from_slice(&secret.to_bytes()[..]), hash).expect("sign message");

        let mut sig: [u8; 65] = [0; 65];
        sig[0..32].copy_from_slice(&signature.r().to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&signature.s().to_be_bytes::<32>());
        sig[64] = signature.v() as u8;

        assert_eq!(recover_signer_unchecked(&sig, &hash).ok(), Some(signer));
    }

    #[test]
    #[cfg(all(feature = "secp256k1", feature = "k256"))]
    fn sanity_secp256k1_k256_compat() {
        use super::{impl_k256, impl_secp256k1};
        use alloy_primitives::B256;

        let (secp256k1_secret, secp256k1_public) =
            secp256k1::generate_keypair(&mut rand::thread_rng());
        let k256_secret = k256::ecdsa::SigningKey::from_slice(&secp256k1_secret.secret_bytes())
            .expect("k256 secret");
        let k256_public = *k256_secret.verifying_key();

        let secp256k1_signer = impl_secp256k1::public_key_to_address(secp256k1_public);
        let k256_signer = impl_k256::public_key_to_address(k256_public);
        assert_eq!(secp256k1_signer, k256_signer);

        let message = b"hello world";
        let hash = alloy_primitives::keccak256(message);

        let secp256k1_signature = impl_secp256k1::sign_message(
            B256::from_slice(&secp256k1_secret.secret_bytes()[..]),
            hash,
        )
        .expect("secp256k1 sign");
        let k256_signature =
            impl_k256::sign_message(B256::from_slice(&k256_secret.to_bytes()[..]), hash)
                .expect("k256 sign");
        assert_eq!(secp256k1_signature, k256_signature);

        let mut sig: [u8; 65] = [0; 65];

        sig[0..32].copy_from_slice(&secp256k1_signature.r().to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&secp256k1_signature.s().to_be_bytes::<32>());
        sig[64] = secp256k1_signature.v() as u8;
        let secp256k1_recovered =
            impl_secp256k1::recover_signer_unchecked(&sig, &hash).expect("secp256k1 recover");
        assert_eq!(secp256k1_recovered, secp256k1_signer);

        sig[0..32].copy_from_slice(&k256_signature.r().to_be_bytes::<32>());
        sig[32..64].copy_from_slice(&k256_signature.s().to_be_bytes::<32>());
        sig[64] = k256_signature.v() as u8;
        let k256_recovered =
            impl_k256::recover_signer_unchecked(&sig, &hash).expect("k256 recover");
        assert_eq!(k256_recovered, k256_signer);

        assert_eq!(secp256k1_recovered, k256_recovered);
    }
}
