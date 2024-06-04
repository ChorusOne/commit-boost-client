use std::collections::HashMap;

use alloy_rpc_types_beacon::{BlsPublicKey, BlsSignature};
use blst::min_pk::SecretKey;
use cb_common::types::Chain;

use crate::{
    error::SignError,
    signature::{random_secret, sign_builder_message},
    types::{ObjectTreeHash, ProxyDelegation, SignedProxyDelegation},
    utils::blst_pubkey_to_alloy,
};

pub enum Signer {
    Plain(SecretKey),
}

impl Signer {
    pub fn new_random() -> Self {
        Signer::Plain(random_secret())
    }

    pub fn new_from_bytes(bytes: &[u8]) -> Self {
        let secret_key = SecretKey::from_bytes(bytes).unwrap();
        Self::Plain(secret_key)
    }

    pub fn pubkey(&self) -> BlsPublicKey {
        match self {
            Signer::Plain(secret) => blst_pubkey_to_alloy(&secret.sk_to_pk()),
        }
    }

    pub async fn sign(&self, chain: Chain, msg: &impl ObjectTreeHash) -> BlsSignature {
        match self {
            Signer::Plain(sk) => sign_builder_message(chain, sk, msg),
        }
    }
}

// For extra safety and to avoid risking signing malicious messages, use a proxy setup:
// proposer creates a new ephemeral keypair which will be used to sign commit messages,
// it also signs a ProxyDelegation associating the new keypair with its consensus pubkey
// When a new commit module starts, pass the ProxyDelegation msg and then sign all future
// commit messages with the proxy key
// for slashing the faulty message + proxy delegation can be used
// Signed using builder domain

pub struct ProxySigner {
    signer: Signer,
    delegation: SignedProxyDelegation,
}

pub struct SigningManager {
    chain: Chain,
    consensus_signers: HashMap<BlsPublicKey, Signer>,
    proxy_signers: HashMap<BlsPublicKey, ProxySigner>,
}

impl SigningManager {
    pub fn new(chain: Chain) -> Self {
        Self { chain, consensus_signers: HashMap::new(), proxy_signers: HashMap::new() }
    }

    pub fn add_consensus_signer(&mut self, signer: Signer) {
        self.consensus_signers.insert(signer.pubkey(), signer);
    }

    pub fn add_proxy_signer(&mut self, proxy: ProxySigner) {
        self.proxy_signers.insert(proxy.signer.pubkey(), proxy);
    }

    pub async fn create_proxy(
        &mut self,
        delegator: BlsPublicKey,
    ) -> Result<SignedProxyDelegation, SignError> {
        let signer = Signer::new_random();

        let message = ProxyDelegation { delegator, proxy: signer.pubkey() };
        let signature = self.sign_consensus(&delegator, &message).await?;
        let signed_delegation: SignedProxyDelegation = SignedProxyDelegation { signature, message };
        let proxy_signer = ProxySigner { signer, delegation: signed_delegation };

        self.add_proxy_signer(proxy_signer);

        Ok(signed_delegation)
    }

    // TODO: double check what we can actually sign here with different providers eg web3 signer
    pub async fn sign_consensus(
        &self,
        pubkey: &BlsPublicKey,
        msg: &impl ObjectTreeHash,
    ) -> Result<BlsSignature, SignError> {
        let signer =
            self.consensus_signers.get(pubkey).ok_or(SignError::UnknownConsensusSigner(*pubkey))?;
        let signature = signer.sign(self.chain, msg).await;

        Ok(signature)
    }

    pub async fn sign_proxy(
        &self,
        pubkey: &BlsPublicKey,
        msg: &impl ObjectTreeHash,
    ) -> Result<BlsSignature, SignError> {
        let proxy = self.proxy_signers.get(pubkey).ok_or(SignError::UnknownProxySigner(*pubkey))?;
        let signature = proxy.signer.sign(self.chain, msg).await;

        Ok(signature)
    }

    pub fn consensus_pubkeys(&self) -> Vec<BlsPublicKey> {
        self.consensus_signers.keys().cloned().collect()
    }

    pub fn proxy_pubkeys(&self) -> Vec<BlsPublicKey> {
        self.proxy_signers.keys().cloned().collect()
    }

    pub fn delegations(&self) -> Vec<SignedProxyDelegation> {
        self.proxy_signers.values().map(|s| s.delegation).collect()
    }

    pub fn has_consensus(&self, pubkey: &BlsPublicKey) -> bool {
        self.consensus_signers.contains_key(pubkey)
    }

    pub fn has_proxy(&self, pubkey: &BlsPublicKey) -> bool {
        self.proxy_signers.contains_key(pubkey)
    }

    pub fn get_delegation(
        &self,
        proxy_pubkey: &BlsPublicKey,
    ) -> Result<SignedProxyDelegation, SignError> {
        let signer = self
            .proxy_signers
            .get(proxy_pubkey)
            .ok_or(SignError::UnknownProxySigner(*proxy_pubkey))?;
        Ok(signer.delegation)
    }
}