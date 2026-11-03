#![no_std]

//! BANJEE per-user wallet.
//!
//! Holds SAC tokens (USDC, native XLM SAC, etc.) at this contract address.
//! Unique receive address = this contract id.
//!
//! Spend paths (caller passes `spender` and signs):
//! - linked `owner` (user Stellar key)
//! - `operator` (backend after WhatsApp PIN)
//!
//! `send_to_naira_vault` sweeps to the configured vault for fiat off-ramp.

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, BytesN, Env,
};

const INSTANCE_LIFETIME_THRESHOLD: u32 = 17_280; // ~1 day
const INSTANCE_BUMP_AMOUNT: u32 = 120_960; // ~7 days
const SECONDS_PER_DAY: u64 = 86_400;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum WalletError {
    NotInitialized = 1,
    Unauthorized = 2,
    InvalidAmount = 3,
    NairaVaultNotSet = 4,
    OwnerAlreadyLinked = 5,
    ExceedsDailyCap = 6,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Factory,
    Owner,
    OwnerLinked,
    Operator,
    WithdrawalAuthority,
    NairaVault,
    DailyCap,
    SpentToday,
    SpendDay,
}

#[contract]
pub struct ZyloWallet;

#[contractimpl]
impl ZyloWallet {
    /// Deploy-time constructor (factory `deploy_v2`).
    ///
    /// Until `link_owner`, only `operator` may spend (WhatsApp custodial mode).
    /// Pass `naira_vault == factory` (or any sentinel equal to this wallet after
    /// deploy) — unset vault is detected when vault == current contract; use
    /// factory address as the "not configured" placeholder at construction.
    pub fn __constructor(
        env: Env,
        factory: Address,
        operator: Address,
        withdrawal_authority: Address,
        naira_vault: Address,
        daily_cap: i128,
    ) {
        env.storage().instance().set(&DataKey::Factory, &factory);
        env.storage().instance().set(&DataKey::Operator, &operator);
        env.storage()
            .instance()
            .set(&DataKey::WithdrawalAuthority, &withdrawal_authority);
        env.storage()
            .instance()
            .set(&DataKey::NairaVault, &naira_vault);
        env.storage().instance().set(&DataKey::DailyCap, &daily_cap);
        env.storage().instance().set(&DataKey::OwnerLinked, &false);
        env.storage().instance().set(&DataKey::SpentToday, &0_i128);
        env.storage().instance().set(&DataKey::SpendDay, &0_u64);
        Self::bump_instance(&env);
    }

    // -----------------------------------------------------------------------
    // Spend
    // -----------------------------------------------------------------------

    /// Transfer SAC tokens to `to`. `spender` must be owner or operator and must authorize.
    pub fn transfer(
        env: Env,
        spender: Address,
        token: Address,
        to: Address,
        amount: i128,
    ) -> Result<(), WalletError> {
        Self::require_spender(&env, &spender)?;
        Self::require_amount(amount)?;
        Self::check_and_update_daily_cap(&env, amount)?;

        token::TokenClient::new(&env, &token).transfer(
            &env.current_contract_address(),
            &to,
            &amount,
        );

        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("xfer"),), (&spender, &token, &to, amount));
        Ok(())
    }

    /// Sweep tokens to the Naira vault for off-ramp settlement (phase 2 fiat).
    pub fn send_to_naira_vault(
        env: Env,
        spender: Address,
        token: Address,
        amount: i128,
    ) -> Result<(), WalletError> {
        Self::require_spender(&env, &spender)?;
        Self::require_amount(amount)?;
        Self::check_and_update_daily_cap(&env, amount)?;

        let vault = Self::load_naira_vault(&env)?;
        // Placeholder: factory address (or self) means vault not configured.
        if vault == env.current_contract_address() {
            return Err(WalletError::NairaVaultNotSet);
        }
        let factory = Self::load_factory(&env)?;
        if vault == factory {
            return Err(WalletError::NairaVaultNotSet);
        }

        token::TokenClient::new(&env, &token).transfer(
            &env.current_contract_address(),
            &vault,
            &amount,
        );

        Self::bump_instance(&env);
        env.events().publish(
            (symbol_short!("naira"),),
            (&spender, &token, &vault, amount),
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Ownership / config
    // -----------------------------------------------------------------------

    /// Link a Stellar address as owner (Freighter / user key). Operator only.
    pub fn link_owner(env: Env, owner: Address) -> Result<(), WalletError> {
        let operator = Self::load_operator(&env)?;
        operator.require_auth();

        let linked: bool = env
            .storage()
            .instance()
            .get(&DataKey::OwnerLinked)
            .unwrap_or(false);
        if linked {
            return Err(WalletError::OwnerAlreadyLinked);
        }

        env.storage().instance().set(&DataKey::Owner, &owner);
        env.storage().instance().set(&DataKey::OwnerLinked, &true);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("owner"),), &owner);
        Ok(())
    }

    /// Current owner transfers ownership. Owner must authorize.
    pub fn transfer_ownership(env: Env, new_owner: Address) -> Result<(), WalletError> {
        let linked: bool = env
            .storage()
            .instance()
            .get(&DataKey::OwnerLinked)
            .unwrap_or(false);
        if !linked {
            return Err(WalletError::Unauthorized);
        }
        let owner = Self::load_owner(&env)?;
        owner.require_auth();
        env.storage().instance().set(&DataKey::Owner, &new_owner);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("own_xfer"),), (&owner, &new_owner));
        Ok(())
    }

    pub fn set_operator(env: Env, operator: Address) -> Result<(), WalletError> {
        Self::require_factory(&env)?;
        env.storage().instance().set(&DataKey::Operator, &operator);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("operator"),), &operator);
        Ok(())
    }

    pub fn set_withdrawal_authority(env: Env, authority: Address) -> Result<(), WalletError> {
        Self::require_factory(&env)?;
        env.storage()
            .instance()
            .set(&DataKey::WithdrawalAuthority, &authority);
        Self::bump_instance(&env);
        Ok(())
    }

    pub fn set_naira_vault(env: Env, vault: Address) -> Result<(), WalletError> {
        Self::require_factory(&env)?;
        env.storage().instance().set(&DataKey::NairaVault, &vault);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("vault"),), &vault);
        Ok(())
    }

    /// `daily_cap == 0` disables the cap. Units are token base units (caller’s convention).
    pub fn set_daily_cap(env: Env, daily_cap: i128) -> Result<(), WalletError> {
        Self::require_factory(&env)?;
        if daily_cap < 0 {
            return Err(WalletError::InvalidAmount);
        }
        env.storage().instance().set(&DataKey::DailyCap, &daily_cap);
        Self::bump_instance(&env);
        Ok(())
    }

    /// Upgrade this wallet Wasm. Factory must authorize (factory invokes this).
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) -> Result<(), WalletError> {
        Self::require_factory(&env)?;
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash);
        Self::bump_instance(&env);
        env.events()
            .publish((symbol_short!("upgrade"),), ());
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Views
    // -----------------------------------------------------------------------

    pub fn get_factory(env: Env) -> Result<Address, WalletError> {
        Self::load_factory(&env)
    }

    pub fn get_operator(env: Env) -> Result<Address, WalletError> {
        Self::load_operator(&env)
    }

    pub fn get_withdrawal_authority(env: Env) -> Result<Address, WalletError> {
        env.storage()
            .instance()
            .get(&DataKey::WithdrawalAuthority)
            .ok_or(WalletError::NotInitialized)
    }

    pub fn get_owner(env: Env) -> Option<Address> {
        let linked: bool = env
            .storage()
            .instance()
            .get(&DataKey::OwnerLinked)
            .unwrap_or(false);
        if !linked {
            return None;
        }
        env.storage().instance().get(&DataKey::Owner)
    }

    pub fn is_owner_linked(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::OwnerLinked)
            .unwrap_or(false)
    }

    pub fn get_naira_vault(env: Env) -> Result<Address, WalletError> {
        Self::load_naira_vault(&env)
    }

    pub fn get_daily_cap(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::DailyCap)
            .unwrap_or(0)
    }

    pub fn token_balance(env: Env, token: Address) -> i128 {
        token::TokenClient::new(&env, &token).balance(&env.current_contract_address())
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    fn require_amount(amount: i128) -> Result<(), WalletError> {
        if amount <= 0 {
            return Err(WalletError::InvalidAmount);
        }
        Ok(())
    }

    fn require_spender(env: &Env, spender: &Address) -> Result<(), WalletError> {
        spender.require_auth();

        let operator = Self::load_operator(env)?;
        if *spender == operator {
            return Ok(());
        }

        let linked: bool = env
            .storage()
            .instance()
            .get(&DataKey::OwnerLinked)
            .unwrap_or(false);
        if linked {
            let owner = Self::load_owner(env)?;
            if *spender == owner {
                return Ok(());
            }
        }

        Err(WalletError::Unauthorized)
    }

    fn check_and_update_daily_cap(env: &Env, amount: i128) -> Result<(), WalletError> {
        let cap: i128 = env
            .storage()
            .instance()
            .get(&DataKey::DailyCap)
            .unwrap_or(0);
        if cap == 0 {
            return Ok(());
        }
        let day = env.ledger().timestamp() / SECONDS_PER_DAY;
        let mut spend_day: u64 = env
            .storage()
            .instance()
            .get(&DataKey::SpendDay)
            .unwrap_or(0);
        let mut spent: i128 = env
            .storage()
            .instance()
            .get(&DataKey::SpentToday)
            .unwrap_or(0);
        if day > spend_day {
            spend_day = day;
            spent = 0;
        }
        let new_spent = spent.checked_add(amount).ok_or(WalletError::ExceedsDailyCap)?;
        if new_spent > cap {
            return Err(WalletError::ExceedsDailyCap);
        }
        env.storage().instance().set(&DataKey::SpendDay, &spend_day);
        env.storage().instance().set(&DataKey::SpentToday, &new_spent);
        Ok(())
    }

    fn require_factory(env: &Env) -> Result<(), WalletError> {
        let factory = Self::load_factory(env)?;
        factory.require_auth();
        Ok(())
    }

    fn load_factory(env: &Env) -> Result<Address, WalletError> {
        env.storage()
            .instance()
            .get(&DataKey::Factory)
            .ok_or(WalletError::NotInitialized)
    }

    fn load_operator(env: &Env) -> Result<Address, WalletError> {
        env.storage()
            .instance()
            .get(&DataKey::Operator)
            .ok_or(WalletError::NotInitialized)
    }

    fn load_owner(env: &Env) -> Result<Address, WalletError> {
        env.storage()
            .instance()
            .get(&DataKey::Owner)
            .ok_or(WalletError::Unauthorized)
    }

    fn load_naira_vault(env: &Env) -> Result<Address, WalletError> {
        env.storage()
            .instance()
            .get(&DataKey::NairaVault)
            .ok_or(WalletError::NairaVaultNotSet)
    }

    fn bump_instance(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::testutils::{Address as _, Ledger};
    use soroban_sdk::{token, Env};

    fn setup() -> (
        Env,
        Address,
        Address,
        Address,
        Address,
        Address,
        ZyloWalletClient<'static>,
    ) {
        let env = Env::default();
        env.mock_all_auths();

        let factory = Address::generate(&env);
        let operator = Address::generate(&env);
        let authority = Address::generate(&env);
        // Placeholder vault = factory (unset)
        let naira_placeholder = factory.clone();

        let token_admin = Address::generate(&env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_addr = token_contract.address();

        let wallet_id = env.register(
            ZyloWallet,
            (
                factory.clone(),
                operator.clone(),
                authority.clone(),
                naira_placeholder,
                0_i128,
            ),
        );
        let client = ZyloWalletClient::new(&env, &wallet_id);

        // Fund the wallet
        let sac = token::StellarAssetClient::new(&env, &token_addr);
        sac.mint(&wallet_id, &1_000_000_000);

        (env, factory, operator, authority, token_addr, wallet_id, client)
    }

    #[test]
    fn operator_can_transfer() {
        let (env, _factory, operator, _auth, token_addr, _wallet, client) = setup();
        let recipient = Address::generate(&env);

        client.transfer(&operator, &token_addr, &recipient, &100_000_000);

        let bal = token::TokenClient::new(&env, &token_addr).balance(&recipient);
        assert_eq!(bal, 100_000_000);
    }

    #[test]
    fn linked_owner_can_transfer() {
        let (env, _factory, _operator, _auth, token_addr, _wallet, client) = setup();
        let owner = Address::generate(&env);
        let recipient = Address::generate(&env);

        client.link_owner(&owner);
        client.transfer(&owner, &token_addr, &recipient, &50_000_000);

        assert_eq!(
            token::TokenClient::new(&env, &token_addr).balance(&recipient),
            50_000_000
        );
        assert!(client.is_owner_linked());
        assert_eq!(client.get_owner(), Some(owner));
    }

    #[test]
    fn stranger_cannot_transfer() {
        let (env, _factory, _operator, _auth, token_addr, _wallet, client) = setup();
        let stranger = Address::generate(&env);
        let recipient = Address::generate(&env);

        let result = client.try_transfer(&stranger, &token_addr, &recipient, &1);
        assert_eq!(result, Err(Ok(WalletError::Unauthorized)));
    }

    #[test]
    fn naira_vault_requires_config() {
        let (_env, _factory, operator, _auth, token_addr, _wallet, client) = setup();
        let result = client.try_send_to_naira_vault(&operator, &token_addr, &1_000);
        assert_eq!(result, Err(Ok(WalletError::NairaVaultNotSet)));
    }

    #[test]
    fn naira_vault_transfer_works() {
        let (env, factory, operator, _auth, token_addr, _wallet, client) = setup();
        let vault = Address::generate(&env);

        // set_naira_vault requires factory auth — mock_all_auths covers it when
        // we invoke as if factory authorized. Direct call uses require_factory.
        // In unit test without factory contract, invoke via client after mocking:
        // We need factory.require_auth — mock_all_auths satisfies any require_auth.
        // But set_naira_vault calls factory.require_auth() where factory is stored address.
        // mock_all_auths should still work.

        // Re-register path: call set through invoking with factory as auth subject.
        // The function does factory.require_auth() — with mock_all_auths OK.
        let wallet_client = &client;
        // Use invoke from test — Client set_naira_vault
        // Actually ZyloWalletClient includes set_naira_vault from contractimpl.
        wallet_client.set_naira_vault(&vault);

        wallet_client.send_to_naira_vault(&operator, &token_addr, &25_000_000);
        assert_eq!(
            token::TokenClient::new(&env, &token_addr).balance(&vault),
            25_000_000
        );
        let _ = factory;
    }

    #[test]
    fn daily_cap_enforced() {
        let env = Env::default();
        env.mock_all_auths();
        env.ledger().with_mut(|li| li.timestamp = 86_400 * 10);

        let factory = Address::generate(&env);
        let operator = Address::generate(&env);
        let authority = Address::generate(&env);

        let token_admin = Address::generate(&env);
        let token_addr = env
            .register_stellar_asset_contract_v2(token_admin)
            .address();

        let wallet_id = env.register(
            ZyloWallet,
            (
                factory.clone(),
                operator.clone(),
                authority,
                factory.clone(),
                100_i128, // cap
            ),
        );
        let client = ZyloWalletClient::new(&env, &wallet_id);
        token::StellarAssetClient::new(&env, &token_addr).mint(&wallet_id, &1_000);

        let to = Address::generate(&env);
        client.transfer(&operator, &token_addr, &to, &60);
        let result = client.try_transfer(&operator, &token_addr, &to, &50);
        assert_eq!(result, Err(Ok(WalletError::ExceedsDailyCap)));
    }

    #[test]
    fn double_link_owner_fails() {
        let (env, _f, _op, _a, _t, _w, client) = setup();
        let owner = Address::generate(&env);
        client.link_owner(&owner);
        let result = client.try_link_owner(&Address::generate(&env));
        assert_eq!(result, Err(Ok(WalletError::OwnerAlreadyLinked)));
    }
}
