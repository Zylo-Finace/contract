#![no_std]

//! BANJEE wallet factory — upgradable registry that deploys a unique wallet per user.
//!
//! Flow: `initialize` → `set_wallet_wasm` (uploaded Wasm hash) → `register(user_id)`
//! returns a deterministic wallet contract address used for deposits.

use soroban_sdk::{
    contract, contractclient, contracterror, contractimpl, contracttype, symbol_short, Address,
    Bytes, BytesN, Env,
};

const INSTANCE_LIFETIME_THRESHOLD: u32 = 17_280;
const INSTANCE_BUMP_AMOUNT: u32 = 120_960;
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 120_960;
const PERSISTENT_BUMP_AMOUNT: u32 = 518_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum FactoryError {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    Unauthorized = 3,
    AlreadyRegistered = 4,
    WalletWasmNotSet = 5,
    UnknownWallet = 6,
    InvalidAmount = 7,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Operator,
    WithdrawalAuthority,
    NairaVault,
    DailyCap,
    WalletWasm,
    UserWallet(Bytes),
    WalletUser(Address),
}

/// Minimal wallet surface invoked by the factory after deploy.
#[contractclient(name = "WalletClient")]
pub trait WalletClientTrait {
    fn upgrade(new_wasm_hash: BytesN<32>);
    fn set_operator(operator: Address);
    fn set_withdrawal_authority(authority: Address);
    fn set_naira_vault(vault: Address);
    fn set_daily_cap(daily_cap: i128);
    fn link_owner(owner: Address);
}

#[contract]
pub struct ZyloFactory;

#[contractimpl]
impl ZyloFactory {
    /// One-time setup. `naira_vault` may equal `admin` as a placeholder until configured.
    pub fn initialize(
        env: Env,
        admin: Address,
        operator: Address,
        withdrawal_authority: Address,
        naira_vault: Address,
        daily_cap: i128,
    ) -> Result<(), FactoryError> {
        if env.storage().instance().has(&DataKey::Admin) {
            return Err(FactoryError::AlreadyInitialized);
        }
        if daily_cap < 0 {
            return Err(FactoryError::InvalidAmount);
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Operator, &operator);
        env.storage()
            .instance()
            .set(&DataKey::WithdrawalAuthority, &withdrawal_authority);
        env.storage()
            .instance()
            .set(&DataKey::NairaVault, &naira_vault);
        env.storage().instance().set(&DataKey::DailyCap, &daily_cap);
        Self::bump_instance(&env);

        env.events().publish(
            (symbol_short!("init"),),
            (&admin, &operator, &withdrawal_authority, daily_cap),
        );
        Ok(())
    }

    /// Set / rotate the Wasm hash used for new wallet deploys. Admin only.
    /// Upload Wasm on-chain first, then pass its hash here.
    pub fn set_wallet_wasm(env: Env, wasm_hash: BytesN<32>) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::WalletWasm, &wasm_hash);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("wasm"),), &wasm_hash);
        Ok(())
    }

    /// Register a user and deploy their wallet. Operator or admin.
    ///
    /// `user_id` should be a stable off-chain id (e.g. hash of phone E.164).
    /// Returns the wallet contract address (unique deposit address).
    pub fn register(env: Env, user_id: Bytes) -> Result<Address, FactoryError> {
        Self::require_operator_or_admin(&env)?;
        Self::require_init(&env)?;

        let user_key = DataKey::UserWallet(user_id.clone());
        if env.storage().persistent().has(&user_key) {
            return Err(FactoryError::AlreadyRegistered);
        }

        let wasm_hash: BytesN<32> = env
            .storage()
            .instance()
            .get(&DataKey::WalletWasm)
            .ok_or(FactoryError::WalletWasmNotSet)?;

        let operator: Address = Self::load_operator(&env)?;
        let authority: Address = env
            .storage()
            .instance()
            .get(&DataKey::WithdrawalAuthority)
            .ok_or(FactoryError::NotInitialized)?;
        let naira_vault: Address = env
            .storage()
            .instance()
            .get(&DataKey::NairaVault)
            .ok_or(FactoryError::NotInitialized)?;
        let daily_cap: i128 = env
            .storage()
            .instance()
            .get(&DataKey::DailyCap)
            .unwrap_or(0);

        let salt = env.crypto().sha256(&user_id);
        let factory_addr = env.current_contract_address();

        let wallet = env
            .deployer()
            .with_current_contract(salt)
            .deploy_v2(
                wasm_hash,
                (
                    factory_addr,
                    operator.clone(),
                    authority,
                    naira_vault,
                    daily_cap,
                ),
            );

        env.storage().persistent().set(&user_key, &wallet);
        env.storage()
            .persistent()
            .set(&DataKey::WalletUser(wallet.clone()), &user_id);
        Self::bump_persistent(&env, &user_key);
        Self::bump_persistent(&env, &DataKey::WalletUser(wallet.clone()));
        Self::bump_instance(&env);

        env.events()
            .publish((symbol_short!("register"), user_id), &wallet);
        Ok(wallet)
    }

    /// Deterministic wallet address for a user_id (before or after deploy).
    pub fn predict_wallet(env: Env, user_id: Bytes) -> Address {
        let salt = env.crypto().sha256(&user_id);
        env.deployer()
            .with_current_contract(salt)
            .deployed_address()
    }

    // -----------------------------------------------------------------------
    // Admin config
    // -----------------------------------------------------------------------

    pub fn set_admin(env: Env, new_admin: Address) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("admin"),), &new_admin);
        Ok(())
    }

    pub fn set_operator(env: Env, new_operator: Address) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::Operator, &new_operator);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("operator"),), &new_operator);
        Ok(())
    }

    pub fn set_withdrawal_authority(
        env: Env,
        authority: Address,
    ) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        env.storage()
            .instance()
            .set(&DataKey::WithdrawalAuthority, &authority);
        Self::bump_instance(&env);
        Ok(())
    }

    pub fn set_naira_vault(env: Env, vault: Address) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        env.storage().instance().set(&DataKey::NairaVault, &vault);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("vault"),), &vault);
        Ok(())
    }

    pub fn set_daily_cap(env: Env, daily_cap: i128) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        if daily_cap < 0 {
            return Err(FactoryError::InvalidAmount);
        }
        env.storage().instance().set(&DataKey::DailyCap, &daily_cap);
        Self::bump_instance(&env);
        Ok(())
    }

    /// Upgrade the factory Wasm itself.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("upgrade"),), ());
        Ok(())
    }

    /// Upgrade an already-deployed user wallet to a new Wasm hash.
    pub fn upgrade_wallet(
        env: Env,
        wallet: Address,
        new_wasm_hash: BytesN<32>,
    ) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        if !env
            .storage()
            .persistent()
            .has(&DataKey::WalletUser(wallet.clone()))
        {
            return Err(FactoryError::UnknownWallet);
        }
        WalletClient::new(&env, &wallet).upgrade(&new_wasm_hash);
        Self::bump_instance(&env);
        Ok(())
    }

    /// Push a new operator onto an existing wallet.
    pub fn push_wallet_operator(
        env: Env,
        wallet: Address,
        operator: Address,
    ) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        Self::require_known_wallet(&env, &wallet)?;
        WalletClient::new(&env, &wallet).set_operator(&operator);
        Ok(())
    }

    /// Push Naira vault config onto an existing wallet.
    pub fn push_wallet_naira_vault(
        env: Env,
        wallet: Address,
        vault: Address,
    ) -> Result<(), FactoryError> {
        Self::require_admin(&env)?;
        Self::require_known_wallet(&env, &wallet)?;
        WalletClient::new(&env, &wallet).set_naira_vault(&vault);
        Ok(())
    }

    /// Link a Stellar owner key on a wallet ( Freighter ). Operator.
    pub fn link_wallet_owner(
        env: Env,
        wallet: Address,
        owner: Address,
    ) -> Result<(), FactoryError> {
        Self::require_operator_or_admin(&env)?;
        Self::require_known_wallet(&env, &wallet)?;
        // Wallet.link_owner requires operator auth — operator must be in auth tree.
        WalletClient::new(&env, &wallet).link_owner(&owner);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Views
    // -----------------------------------------------------------------------

    pub fn get_wallet(env: Env, user_id: Bytes) -> Option<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::UserWallet(user_id))
    }

    pub fn get_user(env: Env, wallet: Address) -> Option<Bytes> {
        env.storage()
            .persistent()
            .get(&DataKey::WalletUser(wallet))
    }

    pub fn is_wallet(env: Env, wallet: Address) -> bool {
        env.storage()
            .persistent()
            .has(&DataKey::WalletUser(wallet))
    }

    pub fn get_admin(env: Env) -> Result<Address, FactoryError> {
        Self::load_admin(&env)
    }

    pub fn get_operator(env: Env) -> Result<Address, FactoryError> {
        Self::load_operator(&env)
    }

    pub fn get_wallet_wasm(env: Env) -> Option<BytesN<32>> {
        env.storage().instance().get(&DataKey::WalletWasm)
    }

    pub fn get_naira_vault(env: Env) -> Result<Address, FactoryError> {
        env.storage()
            .instance()
            .get(&DataKey::NairaVault)
            .ok_or(FactoryError::NotInitialized)
    }

    pub fn get_daily_cap(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::DailyCap)
            .unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    fn require_init(env: &Env) -> Result<(), FactoryError> {
        if !env.storage().instance().has(&DataKey::Admin) {
            return Err(FactoryError::NotInitialized);
        }
        Ok(())
    }

    fn require_admin(env: &Env) -> Result<(), FactoryError> {
        let admin = Self::load_admin(env)?;
        admin.require_auth();
        Ok(())
    }

    fn require_operator_or_admin(env: &Env) -> Result<(), FactoryError> {
        // WhatsApp backend signs as operator. Admin can `set_operator` to themselves
        // if they need to call register directly.
        let operator = Self::load_operator(env)?;
        operator.require_auth();
        Ok(())
    }

    fn require_known_wallet(env: &Env, wallet: &Address) -> Result<(), FactoryError> {
        if !env
            .storage()
            .persistent()
            .has(&DataKey::WalletUser(wallet.clone()))
        {
            return Err(FactoryError::UnknownWallet);
        }
        Ok(())
    }

    fn load_admin(env: &Env) -> Result<Address, FactoryError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(FactoryError::NotInitialized)
    }

    fn load_operator(env: &Env) -> Result<Address, FactoryError> {
        env.storage()
            .instance()
            .get(&DataKey::Operator)
            .ok_or(FactoryError::NotInitialized)
    }

    fn bump_instance(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    fn bump_persistent(env: &Env, key: &DataKey) {
        env.storage().persistent().extend_ttl(
            key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::Address as _;
    use soroban_sdk::{token, Bytes, Env};

    // Built by: `stellar contract build` in this workspace (wallet package).
    // Path is relative to this crate's manifest when tests run from workspace.
    const WALLET_WASM: &[u8] = include_bytes!("../../target/wasm32v1-none/release/zylo_wallet.wasm");

    fn setup_factory(env: &Env) -> (Address, Address, Address, ZyloFactoryClient<'static>) {
        let admin = Address::generate(env);
        let operator = Address::generate(env);
        let authority = Address::generate(env);

        let factory_id = env.register(ZyloFactory, ());
        let client = ZyloFactoryClient::new(env, &factory_id);

        env.mock_all_auths();
        client.initialize(
            &admin,
            &operator,
            &authority,
            &admin, // naira placeholder
            &0_i128,
        );

        let wasm_hash = env.deployer().upload_contract_wasm(WALLET_WASM);
        client.set_wallet_wasm(&wasm_hash);

        (admin, operator, authority, client)
    }

    #[test]
    fn register_deploys_unique_wallet() {
        let env = Env::default();
        let (_admin, operator, _authority, client) = setup_factory(&env);

        let user_id = Bytes::from_slice(&env, b"whatsapp:+2348168882530");
        let predicted = client.predict_wallet(&user_id);
        let wallet = client.register(&user_id);

        assert_eq!(wallet, predicted);
        assert_eq!(client.get_wallet(&user_id), Some(wallet.clone()));
        assert!(client.is_wallet(&wallet));
        assert_eq!(client.get_user(&wallet), Some(user_id));

        // Wallet should be operable by operator: fund + transfer
        let token_admin = Address::generate(&env);
        let token_addr = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();
        token::StellarAssetClient::new(&env, &token_addr).mint(&wallet, &500_000_000);

        // Invoke wallet transfer via zylo_wallet client from path dep
        let w = zylo_wallet::ZyloWalletClient::new(&env, &wallet);
        assert_eq!(w.get_operator(), operator);
        assert!(!w.is_owner_linked());

        let to = Address::generate(&env);
        w.transfer(&operator, &token_addr, &to, &10_000_000);
        assert_eq!(
            token::TokenClient::new(&env, &token_addr).balance(&to),
            10_000_000
        );
    }

    #[test]
    fn double_register_fails() {
        let env = Env::default();
        let (_admin, _op, _auth, client) = setup_factory(&env);
        let user_id = Bytes::from_slice(&env, b"user-1");
        client.register(&user_id);
        let result = client.try_register(&user_id);
        assert_eq!(result, Err(Ok(FactoryError::AlreadyRegistered)));
    }

    #[test]
    fn different_users_get_different_wallets() {
        let env = Env::default();
        let (_admin, _op, _auth, client) = setup_factory(&env);
        let a = client.register(&Bytes::from_slice(&env, b"a"));
        let b = client.register(&Bytes::from_slice(&env, b"b"));
        assert_ne!(a, b);
    }
}
