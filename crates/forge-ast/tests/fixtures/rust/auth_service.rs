use std::sync::Arc;

pub mod nested;

pub const TOKEN_TTL_SECONDS: u64 = 3600;

pub static AUTH_HEADER: &str = "authorization";

pub struct AuthService {
    store: Arc<String>,
}

pub enum AuthError {
    Missing,
    Invalid,
}

pub trait TokenVerifier {
    fn verify(&self, token: &str) -> bool;
}

impl AuthService {
    pub fn new(store: Arc<String>) -> Self {
        Self { store }
    }

    pub fn authenticate(&self, token: &str) -> Result<(), AuthError> {
        validate_token(token);
        Ok(())
    }
}

pub fn validate_token(token: &str) -> bool {
    !token.is_empty()
}
