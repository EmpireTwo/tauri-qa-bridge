// Copyright (c) 2025 Mathieu Piton
// Copyright (c) 2026 tauri-pilot contributors

use crate::error::Error;
use crate::protocol::{Request, Response};

#[derive(Debug, Clone)]
pub(crate) struct AuthToken(String);

impl AuthToken {
    pub(crate) fn create(identifier: &str) -> Result<Self, Error> {
        let mut bytes = [0_u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| Error::Io(std::io::Error::other(e.to_string())))?;
        let token = bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = token_file_path(identifier);
        write_private_file(&path, token.as_bytes())?;
        Ok(Self(token))
    }

    fn matches(&self, provided: &str) -> bool {
        self.0 == provided
    }
}

fn discovery_dir(identifier: &str) -> std::path::PathBuf {
    std::env::temp_dir().join("tauri-pilot").join(identifier)
}

pub fn token_file_path(identifier: &str) -> std::path::PathBuf {
    discovery_dir(identifier).join("pilot.token")
}

pub(crate) enum Handshake {
    Accepted(Response),
    Rejected(Response),
}

pub(crate) fn validate_handshake(req: &Request, token: &AuthToken) -> Handshake {
    if req.method != "auth.handshake" {
        return Handshake::Rejected(Response::error(
            serde_json::Value::Number(req.id.into()),
            -32001,
            "unauthorized: first request must be auth.handshake",
        ));
    }

    let provided = req
        .params
        .as_ref()
        .and_then(|params| params.get("token"))
        .and_then(serde_json::Value::as_str);

    if provided.is_some_and(|provided| token.matches(provided)) {
        Handshake::Accepted(Response::success(
            req.id,
            serde_json::json!({"authenticated": true}),
        ))
    } else {
        Handshake::Rejected(Response::error(
            serde_json::Value::Number(req.id.into()),
            -32001,
            "unauthorized: bad pilot token",
        ))
    }
}

fn ensure_private_dir(path: &std::path::Path) -> Result<(), Error> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> Result<(), Error> {
    ensure_private_dir(&std::env::temp_dir().join("tauri-pilot"))?;
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }

    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;
        file.flush()?;
        return Ok(());
    }

    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_auth_bypass_attempt() {
        let token = AuthToken("good".to_owned());
        let req = Request {
            jsonrpc: "2.0".to_owned(),
            id: 1,
            method: "dom.snapshot".to_owned(),
            params: None,
        };

        match validate_handshake(&req, &token) {
            Handshake::Rejected(resp) => {
                assert_eq!(resp.error.expect("error").code, -32001);
            }
            Handshake::Accepted(_) => panic!("bypass accepted"),
        }
    }

    #[test]
    fn accepts_matching_token() {
        let token = AuthToken("good".to_owned());
        let req = Request {
            jsonrpc: "2.0".to_owned(),
            id: 7,
            method: "auth.handshake".to_owned(),
            params: Some(serde_json::json!({"token": "good"})),
        };

        match validate_handshake(&req, &token) {
            Handshake::Accepted(resp) => {
                assert_eq!(
                    resp.result,
                    Some(serde_json::json!({"authenticated": true}))
                );
            }
            Handshake::Rejected(_) => panic!("valid token rejected"),
        }
    }
}
