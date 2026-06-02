use super::models::*;
use crate::database::PgPool;
use rand::Rng;
use rust_decimal::Decimal;
use std::sync::Arc;
use uuid::Uuid;

static BANK_NAMES: &[(&str, &str)] = &[
    ("044", "Access Bank"),
    ("023", "Citibank"),
    ("050", "EcoBank"),
    ("011", "First Bank"),
    ("214", "First City Monument Bank"),
    ("070", "Fidelity Bank"),
    ("058", "GTBank"),
    ("030", "Heritage Bank"),
    ("301", "Jaiz Bank"),
    ("082", "Keystone Bank"),
    ("076", "Polaris Bank"),
    ("221", "Stanbic IBTC"),
    ("068", "Standard Chartered"),
    ("232", "Sterling Bank"),
    ("032", "Union Bank"),
    ("033", "United Bank for Africa"),
    ("215", "Unity Bank"),
    ("035", "Wema Bank"),
    ("057", "Zenith Bank"),
];

#[derive(Clone)]
pub struct DataFactoryService {
    pool: Arc<PgPool>,
}

impl DataFactoryService {
    pub fn new(pool: Arc<PgPool>) -> Self {
        Self { pool }
    }

    pub async fn generate_test_data(
        &self,
        application_id: Uuid,
        req: GenerateTestDataRequest,
    ) -> Result<GenerateTestDataResponse, DeveloperPortalError> {
        let user_count = req.user_count.unwrap_or(3).min(50).max(1) as usize;
        let txns_per_user = req.transactions_per_user.unwrap_or(5).min(20).max(1) as usize;
        let initial_balance = req
            .initial_balance_ngn
            .unwrap_or_else(|| Decimal::new(100_000_00, 2)); // ₦100,000

        let mut users = Vec::with_capacity(user_count);
        let mut bank_accounts_created = 0usize;
        let mut transactions_created = 0usize;

        for i in 0..user_count {
            let user = self
                .create_test_user(application_id, i, initial_balance)
                .await?;

            // One bank account per user
            self.create_test_bank_account(application_id, user.id).await?;
            bank_accounts_created += 1;

            // Mock transactions
            for j in 0..txns_per_user {
                self.create_mock_transaction(application_id, user.id, j)
                    .await?;
                transactions_created += 1;
            }

            users.push(user);
        }

        Ok(GenerateTestDataResponse {
            users_created: user_count,
            bank_accounts_created,
            transactions_created,
            users,
        })
    }

    pub async fn reset_environment(
        &self,
        application_id: Uuid,
    ) -> Result<(), DeveloperPortalError> {
        // Cascade deletes handle bank accounts and transactions via FK
        sqlx::query!(
            "DELETE FROM sandbox_test_users WHERE application_id = $1",
            application_id
        )
        .execute(self.pool.as_ref())
        .await?;

        sqlx::query!(
            "DELETE FROM sandbox_chaos_scenarios WHERE application_id = $1",
            application_id
        )
        .execute(self.pool.as_ref())
        .await?;

        Ok(())
    }

    pub async fn list_test_users(
        &self,
        application_id: Uuid,
    ) -> Result<Vec<SandboxTestUser>, DeveloperPortalError> {
        let users = sqlx::query_as!(
            SandboxTestUser,
            "SELECT * FROM sandbox_test_users WHERE application_id = $1 ORDER BY created_at",
            application_id
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(users)
    }

    pub async fn list_mock_transactions(
        &self,
        application_id: Uuid,
    ) -> Result<Vec<SandboxMockTransaction>, DeveloperPortalError> {
        let txns = sqlx::query_as!(
            SandboxMockTransaction,
            "SELECT * FROM sandbox_mock_transactions WHERE application_id = $1 ORDER BY created_at DESC",
            application_id
        )
        .fetch_all(self.pool.as_ref())
        .await?;
        Ok(txns)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    async fn create_test_user(
        &self,
        application_id: Uuid,
        index: usize,
        balance_ngn: Decimal,
    ) -> Result<SandboxTestUser, DeveloperPortalError> {
        let mut rng = rand::thread_rng();
        let first_names = ["Amara", "Chidi", "Fatima", "Emeka", "Ngozi", "Tunde", "Aisha", "Bola"];
        let last_names = ["Okafor", "Adeyemi", "Ibrahim", "Nwosu", "Bello", "Eze", "Abubakar", "Osei"];

        let first = first_names[rng.gen_range(0..first_names.len())];
        let last = last_names[rng.gen_range(0..last_names.len())];
        let full_name = format!("{} {}", first, last);
        let external_id = format!("test_user_{}", Uuid::new_v4().simple());
        let email = format!("{}_{}.{}@sandbox.aframp.io", first.to_lowercase(), index, last.to_lowercase());
        let stellar_address = self.generate_stellar_testnet_address();

        let user = sqlx::query_as!(
            SandboxTestUser,
            r#"INSERT INTO sandbox_test_users
               (application_id, external_id, full_name, email, phone, kyc_status,
                balance_ngn, balance_cngn, stellar_address, metadata)
               VALUES ($1, $2, $3, $4, $5, 'verified', $6, 0, $7, '{}')
               RETURNING *"#,
            application_id,
            external_id,
            full_name,
            email,
            format!("+234{}", rng.gen_range(7000000000u64..9999999999u64)),
            balance_ngn,
            stellar_address,
        )
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(user)
    }

    async fn create_test_bank_account(
        &self,
        application_id: Uuid,
        test_user_id: Uuid,
    ) -> Result<SandboxTestBankAccount, DeveloperPortalError> {
        let mut rng = rand::thread_rng();
        let (bank_code, bank_name) = BANK_NAMES[rng.gen_range(0..BANK_NAMES.len())];
        let account_number: String = (0..10).map(|_| rng.gen_range(0..10).to_string()).collect();

        let account = sqlx::query_as!(
            SandboxTestBankAccount,
            r#"INSERT INTO sandbox_test_bank_accounts
               (application_id, test_user_id, account_number, bank_code, bank_name, account_name, currency, is_verified)
               VALUES ($1, $2, $3, $4, $5, 'Sandbox Test Account', 'NGN', true)
               RETURNING *"#,
            application_id,
            test_user_id,
            account_number,
            bank_code,
            bank_name,
        )
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(account)
    }

    async fn create_mock_transaction(
        &self,
        application_id: Uuid,
        test_user_id: Uuid,
        index: usize,
    ) -> Result<SandboxMockTransaction, DeveloperPortalError> {
        let mut rng = rand::thread_rng();
        let types = ["onramp", "offramp", "transfer"];
        let statuses = ["completed", "completed", "completed", "failed"]; // 75% success
        let tx_type = types[index % types.len()];
        let status = statuses[rng.gen_range(0..statuses.len())];
        let amount = Decimal::new(rng.gen_range(1_000_00..500_000_00), 2);
        let reference = format!("SANDBOX_{}", Uuid::new_v4().simple().to_string().to_uppercase());
        let stellar_hash = format!("{:064x}", rng.gen::<u128>() as u128 * rng.gen::<u128>() as u128);

        let txn = sqlx::query_as!(
            SandboxMockTransaction,
            r#"INSERT INTO sandbox_mock_transactions
               (application_id, test_user_id, transaction_type, status, amount, currency,
                stellar_tx_hash, reference, metadata)
               VALUES ($1, $2, $3, $4, $5, 'NGN', $6, $7, '{}')
               RETURNING *"#,
            application_id,
            test_user_id,
            tx_type,
            status,
            amount,
            stellar_hash,
            reference,
        )
        .fetch_one(self.pool.as_ref())
        .await?;

        Ok(txn)
    }

    fn generate_stellar_testnet_address(&self) -> String {
        // Stellar public keys start with 'G' and are 56 chars (base32)
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let mut rng = rand::thread_rng();
        let body: String = (0..55)
            .map(|_| alphabet[rng.gen_range(0..alphabet.len())] as char)
            .collect();
        format!("G{}", body)
    }
}
