//! User authentication: SSH agent, key files (passphrases asked only when the server
//! accepts the key), passwords and keyboard-interactive (2FA) — in OpenSSH-like order.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::ssh_encoding::Encode;
use russh::keys::ssh_key::Signature;
use russh::keys::ssh_key::private::KeypairData;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::{MethodKind, MethodSet};
use zeroize::Zeroizing;

use super::handler::ClientHandler;
use super::{KbdPrompt, LogLevel, Prompt, PromptReply, SessionCtx, lock};
use crate::error::{Error, Result};
use crate::i18n;
use crate::keys::{self, KeyFile, UnlockError};
use crate::model::{AuthKind, Server};
use crate::secrets::Secrets;
use crate::util;

type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>;

/// Authenticates `hop` on an established (not yet authenticated) connection. Its password
/// may be remembered in the keychain under `password_account`.
pub(crate) async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    hop: &Server,
    password_account: Option<&str>,
    ctx: &Arc<SessionCtx>,
) -> Result<()> {
    let mut auth = Auth {
        handle,
        ctx,
        hop,
        user: hop.effective_user(),
        password_account: password_account.map(str::to_string),
        remaining: None,
        failed: Vec::new(),
        rsa_hash: None,
    };
    if auth.none().await? {
        return Ok(());
    }
    let done = match hop.auth {
        AuthKind::Agent => auth.agent(None).await?,
        AuthKind::Key => {
            let path = util::expand_tilde(hop.identity_file.as_deref().unwrap_or_default());
            auth.key_file(&path, true).await?
        }
        AuthKind::Password => auth.password().await?,
        AuthKind::Auto => auth.automatic().await?,
    };
    if done {
        Ok(())
    } else if auth.failed.is_empty() {
        Err(Error::AuthFailed(i18n::auth_no_method(&auth.offered())))
    } else {
        Err(Error::AuthFailed(i18n::auth_tried(&auth.failed.join(", "))))
    }
}

struct Auth<'a> {
    handle: &'a mut Handle<ClientHandler>,
    ctx: &'a Arc<SessionCtx>,
    hop: &'a Server,
    user: String,
    /// Where its password is remembered: saved servers and their jump hosts have one.
    password_account: Option<String>,
    /// Methods the server allows to continue with; `None` until it told us.
    remaining: Option<MethodSet>,
    failed: Vec<String>,
    rsa_hash: Option<Option<HashAlg>>,
}

impl Auth<'_> {
    fn allows(&self, method: MethodKind) -> bool {
        self.remaining.as_ref().is_none_or(|r| r.contains(&method))
    }

    fn offered(&self) -> String {
        match &self.remaining {
            Some(set) if !set.is_empty() => set
                .iter()
                .map(|m| <&str>::from(m).to_string())
                .collect::<Vec<_>>()
                .join(", "),
            _ => i18n::auth_nothing_offered(),
        }
    }

    /// Updates state from a result; true on success.
    fn record(&mut self, result: AuthResult, what: &str) -> bool {
        match result {
            AuthResult::Success => {
                self.ctx
                    .log(LogLevel::Info, i18n::authenticated(&self.user, what));
                true
            }
            AuthResult::Failure {
                remaining_methods,
                partial_success,
            } => {
                if partial_success {
                    self.ctx.log(LogLevel::Info, i18n::auth_partial(what));
                } else {
                    self.failed.push(what.to_string());
                }
                self.remaining = Some(remaining_methods);
                false
            }
        }
    }

    async fn none(&mut self) -> Result<bool> {
        match self.handle.authenticate_none(&self.user).await? {
            AuthResult::Success => {
                self.ctx
                    .log(LogLevel::Info, i18n::authenticated_without_auth());
                Ok(true)
            }
            AuthResult::Failure {
                remaining_methods, ..
            } => {
                self.remaining = Some(remaining_methods);
                Ok(false)
            }
        }
    }

    async fn automatic(&mut self) -> Result<bool> {
        // A remembered password means the user logs in with a password: try it first so
        // unrelated keys do not use up the server's MaxAuthTries.
        if (self.allows(MethodKind::Password) || self.allows(MethodKind::KeyboardInteractive))
            && let Some(known) = self.known_password().await
            && self.password_with(Some(known)).await?
        {
            return Ok(true);
        }
        if self.agent(None).await? {
            return Ok(true);
        }
        let explicit = self.hop.identity_file.as_deref().map(util::expand_tilde);
        if let Some(path) = &explicit
            && self.key_file(path, true).await?
        {
            return Ok(true);
        }
        for path in keys::default_identity_files() {
            if explicit.as_ref() != Some(&path) && self.key_file(&path, false).await? {
                return Ok(true);
            }
        }
        self.password().await
    }

    // ---- public keys -------------------------------------------------------------

    async fn rsa_hash(&mut self) -> Option<HashAlg> {
        if self.rsa_hash.is_none() {
            // `None` from the server means it did not announce `server-sig-algs`,
            // i.e. an old server that only knows `ssh-rsa` (SHA-1).
            self.rsa_hash = Some(
                self.handle
                    .best_supported_rsa_hash()
                    .await
                    .ok()
                    .flatten()
                    .flatten(),
            );
        }
        self.rsa_hash.flatten()
    }

    async fn hash_for(&mut self, key: &PublicKey) -> Option<HashAlg> {
        if key.algorithm().is_rsa() {
            self.rsa_hash().await
        } else {
            None
        }
    }

    /// Tries identities from the SSH agent (only `only` when given).
    async fn agent(&mut self, only: Option<&PublicKey>) -> Result<bool> {
        if !self.allows(MethodKind::PublicKey) {
            return Ok(false);
        }
        let Some(mut agent) = connect_agent().await else {
            return Ok(false);
        };
        let identities =
            match tokio::time::timeout(Duration::from_secs(5), agent.request_identities()).await {
                Ok(Ok(ids)) => ids,
                Ok(Err(e)) => {
                    log::debug!("ssh-agent: {e}");
                    return Ok(false);
                }
                Err(_) => {
                    self.ctx.log(LogLevel::Warn, i18n::agent_no_response());
                    return Ok(false);
                }
            };
        for identity in identities {
            if !self.allows(MethodKind::PublicKey) {
                break;
            }
            let (label, result) = match &identity {
                AgentIdentity::PublicKey { key, comment } => {
                    if only.is_some_and(|k| k.key_data() != key.key_data()) {
                        continue;
                    }
                    let hash = self.hash_for(key).await;
                    let label = agent_label(comment, key);
                    let result = self
                        .handle
                        .authenticate_publickey_with(&self.user, key.clone(), hash, &mut agent)
                        .await;
                    (label, result)
                }
                AgentIdentity::Certificate {
                    certificate,
                    comment,
                } => {
                    if only.is_some() {
                        continue;
                    }
                    let hash = if certificate.algorithm().is_rsa() {
                        self.rsa_hash().await
                    } else {
                        None
                    };
                    let label = i18n::method_agent_certificate(comment);
                    let result = self
                        .handle
                        .authenticate_certificate_with(
                            &self.user,
                            certificate.clone(),
                            hash,
                            &mut agent,
                        )
                        .await;
                    (label, result)
                }
            };
            match result {
                Ok(result) => {
                    if self.record(result, &label) {
                        return Ok(true);
                    }
                }
                // The protocol cannot recover from a failed signature: give up on this connection.
                Err(e) => return Err(Error::AuthFailed(i18n::agent_sign_failed(e))),
            }
        }
        Ok(false)
    }

    /// Tries a private key file. `explicit` keys report problems and may ask for a
    /// passphrase up front; default keys are skipped quietly when unusable.
    async fn key_file(&mut self, path: &Path, explicit: bool) -> Result<bool> {
        if !self.allows(MethodKind::PublicKey) {
            return Ok(false);
        }
        let key = match KeyFile::read(path) {
            Ok(key) => key,
            Err(e) => {
                if explicit {
                    self.ctx.log(LogLevel::Warn, e.to_string());
                }
                return Ok(false);
            }
        };
        let label = i18n::method_key(&key.display_path());

        if !key.encrypted {
            return match key.unlock(None) {
                Ok(private) => {
                    let result = self.publickey(private).await?;
                    Ok(self.record(result, &label))
                }
                Err(_) => Ok(false),
            };
        }

        match key.public.clone() {
            Some(public) => {
                // The key may already be unlocked in the agent: no passphrase needed.
                if self.agent(Some(&public)).await? {
                    return Ok(true);
                }
                if !self.allows(MethodKind::PublicKey) {
                    return Ok(false);
                }
                // Offer the public key first; the passphrase is only asked for if the
                // server accepts it (like OpenSSH).
                let hash = self.hash_for(&public).await;
                let mut signer = PassphraseSigner {
                    ctx: self.ctx,
                    key: &key,
                };
                let result = self
                    .handle
                    .authenticate_publickey_with(&self.user, public, hash, &mut signer)
                    .await
                    .map_err(|e| Error::AuthFailed(e.to_string()))?;
                Ok(self.record(result, &label))
            }
            None if explicit => match unlock_interactive(self.ctx, &key).await? {
                Some(private) => {
                    let result = self.publickey(private).await?;
                    Ok(self.record(result, &label))
                }
                None => Ok(false),
            },
            None => Ok(false),
        }
    }

    async fn publickey(&mut self, key: PrivateKey) -> Result<AuthResult> {
        let hash = self.hash_for(key.public_key()).await;
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        Ok(self.handle.authenticate_publickey(&self.user, key).await?)
    }

    // ---- passwords ---------------------------------------------------------------

    fn cache_key(&self) -> String {
        format!(
            "{}@{}",
            self.user,
            util::host_port(&self.hop.host, self.hop.port)
        )
    }

    async fn known_password(&self) -> Option<Zeroizing<String>> {
        if let Some(p) = lock(&self.ctx.cache).passwords.get(&self.cache_key()) {
            return Some(p.clone());
        }
        if let Some(account) = &self.password_account
            && let Ok(Some(p)) = self.ctx.shared.secrets.get_async(account.clone()).await
        {
            return Some(p);
        }
        None
    }

    async fn ask_password(&self, error: Option<String>) -> Result<(Zeroizing<String>, bool)> {
        let can_remember =
            self.password_account.is_some() && self.ctx.shared.secrets.available_async().await;
        let reply = self
            .ctx
            .ask(Prompt::Password {
                user: self.user.clone(),
                host: short_host(self.hop),
                can_remember,
                error,
            })
            .await;
        match reply {
            Some(PromptReply::Secret { value, remember }) => {
                Ok((Zeroizing::new(value), remember && can_remember))
            }
            _ => Err(Error::Cancelled),
        }
    }

    async fn keep_password(&self, password: Zeroizing<String>, remember: bool) {
        if remember && let Some(account) = self.password_account.clone() {
            match self
                .ctx
                .shared
                .secrets
                .set_async(account, password.clone())
                .await
            {
                Ok(()) => self.ctx.log(LogLevel::Info, i18n::password_saved()),
                Err(e) => self.ctx.log(LogLevel::Warn, i18n::password_save_failed(e)),
            }
        }
        lock(&self.ctx.cache)
            .passwords
            .insert(self.cache_key(), password);
    }

    async fn password(&mut self) -> Result<bool> {
        let known = self.known_password().await;
        self.password_with(known).await
    }

    /// Password or keyboard-interactive login: `first` (a remembered password) is tried
    /// once, then the user is asked up to three times.
    async fn password_with(&mut self, mut first: Option<Zeroizing<String>>) -> Result<bool> {
        let mut error = None;
        for _ in 0..4 {
            let (password_ok, kbd_ok) = (
                self.allows(MethodKind::Password),
                self.allows(MethodKind::KeyboardInteractive),
            );
            if password_ok {
                let (password, remember) = match first.take() {
                    Some(p) => (p, false),
                    None => self.ask_password(error.take()).await?,
                };
                let result = self
                    .handle
                    .authenticate_password(&self.user, password.as_str())
                    .await?;
                if self.record(result, &i18n::method_password()) {
                    self.keep_password(password, remember).await;
                    return Ok(true);
                }
            } else if kbd_ok {
                if self.keyboard_interactive(&mut first, &mut error).await? {
                    return Ok(true);
                }
            } else {
                return Ok(false);
            }
            error = Some(i18n::wrong_password());
        }
        Ok(false)
    }

    /// One keyboard-interactive exchange. Single hidden "password" prompts use the normal
    /// password dialog (and the remembered password); anything else (OTP codes, etc.)
    /// is shown as-is.
    async fn keyboard_interactive(
        &mut self,
        first: &mut Option<Zeroizing<String>>,
        error: &mut Option<String>,
    ) -> Result<bool> {
        let mut response = self
            .handle
            .authenticate_keyboard_interactive_start(&self.user, None::<String>)
            .await?;
        let mut typed: Option<(Zeroizing<String>, bool)> = None;
        for _ in 0..16 {
            match response {
                KeyboardInteractiveAuthResponse::Success => {
                    self.ctx.log(
                        LogLevel::Info,
                        i18n::authenticated(&self.user, &i18n::method_keyboard_interactive()),
                    );
                    if let Some((password, remember)) = typed.take() {
                        self.keep_password(password, remember).await;
                    }
                    return Ok(true);
                }
                KeyboardInteractiveAuthResponse::Failure {
                    remaining_methods,
                    partial_success,
                } => {
                    let result = AuthResult::Failure {
                        remaining_methods,
                        partial_success,
                    };
                    self.record(result, &i18n::method_keyboard_interactive());
                    return Ok(false);
                }
                KeyboardInteractiveAuthResponse::InfoRequest {
                    name,
                    instructions,
                    prompts,
                } => {
                    let answers = if prompts.is_empty() {
                        Vec::new()
                    } else if prompts.len() == 1
                        && !prompts[0].echo
                        && is_password_prompt(&prompts[0].prompt)
                    {
                        let password = match first.take() {
                            Some(p) => p,
                            None => {
                                let (p, remember) = self.ask_password(error.take()).await?;
                                typed = Some((p.clone(), remember));
                                p
                            }
                        };
                        vec![password.to_string()]
                    } else {
                        let prompt = Prompt::KeyboardInteractive {
                            host: format!("{}@{}", self.user, short_host(self.hop)),
                            name,
                            instructions,
                            prompts: prompts
                                .iter()
                                .map(|p| KbdPrompt {
                                    text: p.prompt.clone(),
                                    echo: p.echo,
                                })
                                .collect(),
                        };
                        match self.ctx.ask(prompt).await {
                            Some(PromptReply::Answers { values })
                                if values.len() == prompts.len() =>
                            {
                                values
                            }
                            _ => return Err(Error::Cancelled),
                        }
                    };
                    response = self
                        .handle
                        .authenticate_keyboard_interactive_respond(answers)
                        .await?;
                }
            }
        }
        Ok(false)
    }
}

/// `host` or `host:port` (port omitted when 22), for prompts.
fn short_host(hop: &Server) -> String {
    if hop.port == 22 {
        hop.host.clone()
    } else {
        util::host_port(&hop.host, hop.port)
    }
}

fn is_password_prompt(prompt: &str) -> bool {
    let p = prompt.to_lowercase();
    [
        "password",
        "passwort",
        "пароль",
        "mot de passe",
        "contraseña",
        "senha",
    ]
    .iter()
    .any(|w| p.contains(w))
}

fn agent_label(comment: &str, key: &PublicKey) -> String {
    if comment.trim().is_empty() {
        i18n::method_agent_key(key.fingerprint(HashAlg::Sha256))
    } else {
        i18n::method_agent_key(comment.trim())
    }
}

async fn connect_agent() -> Option<Agent> {
    let connect = async {
        #[cfg(unix)]
        {
            AgentClient::connect_env().await.ok().map(|a| a.dynamic())
        }
        #[cfg(windows)]
        {
            // Windows OpenSSH agent, or a custom agent (1Password, KeePassXC...) via SSH_AUTH_SOCK.
            if let Ok(path) = std::env::var("SSH_AUTH_SOCK")
                && let Ok(a) = AgentClient::connect_named_pipe(&path).await
            {
                return Some(a.dynamic());
            }
            if let Ok(a) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
                return Some(a.dynamic());
            }
            AgentClient::connect_pageant()
                .await
                .ok()
                .map(|a| a.dynamic())
        }
        #[cfg(not(any(unix, windows)))]
        {
            None
        }
    };
    tokio::time::timeout(Duration::from_secs(3), connect)
        .await
        .ok()
        .flatten()
}

/// Unlocks an encrypted key with a passphrase from the session cache, the keychain,
/// or the user (three attempts). `None` if the user gave up.
async fn unlock_interactive(ctx: &Arc<SessionCtx>, key: &KeyFile) -> Result<Option<PrivateKey>> {
    let cached = lock(&ctx.cache).passphrases.get(&key.path).cloned();
    if let Some(pass) = cached
        && let Ok(k) = key.unlock(Some(&pass))
    {
        return Ok(Some(k));
    }
    let account = Secrets::passphrase_account(&key.path);
    if let Ok(Some(pass)) = ctx.shared.secrets.get_async(account.clone()).await
        && let Ok(k) = key.unlock(Some(&pass))
    {
        lock(&ctx.cache).passphrases.insert(key.path.clone(), pass);
        return Ok(Some(k));
    }
    let can_remember = ctx.shared.secrets.available_async().await;
    let mut error = None;
    for _ in 0..3 {
        let reply = ctx
            .ask(Prompt::Passphrase {
                key_path: key.display_path(),
                can_remember,
                error: error.take(),
            })
            .await;
        let Some(PromptReply::Secret { value, remember }) = reply else {
            return Ok(None);
        };
        let pass = Zeroizing::new(value);
        match key.unlock(Some(&pass)) {
            Ok(k) => {
                if remember
                    && can_remember
                    && let Err(e) = ctx
                        .shared
                        .secrets
                        .set_async(account.clone(), pass.clone())
                        .await
                {
                    ctx.log(LogLevel::Warn, i18n::passphrase_save_failed(e));
                }
                lock(&ctx.cache).passphrases.insert(key.path.clone(), pass);
                return Ok(Some(k));
            }
            Err(UnlockError::WrongPassphrase) | Err(UnlockError::NeedsPassphrase) => {
                error = Some(i18n::wrong_passphrase());
            }
            Err(UnlockError::Invalid(e)) => return Err(Error::invalid(e)),
        }
    }
    Ok(None)
}

/// Signs the authentication request with a key that is decrypted only once the server
/// has accepted its public half.
struct PassphraseSigner<'a> {
    ctx: &'a Arc<SessionCtx>,
    key: &'a KeyFile,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct SignerError(String);

impl From<russh::SendError> for SignerError {
    fn from(e: russh::SendError) -> Self {
        SignerError(e.to_string())
    }
}

impl russh::Signer for PassphraseSigner<'_> {
    type Error = SignerError;

    fn auth_sign(
        &mut self,
        identity: &AgentIdentity,
        hash_alg: Option<HashAlg>,
        mut to_sign: Vec<u8>,
    ) -> impl std::future::Future<Output = std::result::Result<Vec<u8>, Self::Error>> + Send {
        let public = match identity {
            AgentIdentity::PublicKey { key, .. } => Some(key.clone()),
            AgentIdentity::Certificate { .. } => None,
        };
        async move {
            let signed = match unlock_interactive(self.ctx, self.key).await {
                Ok(Some(private)) => sign(&private, hash_alg, &to_sign)
                    .map_err(|e| log::warn!("signing with {} failed: {e}", self.key.display_path()))
                    .ok(),
                Ok(None) => None,
                Err(e) => {
                    self.ctx.log(LogLevel::Warn, e.to_string());
                    None
                }
            };
            // russh waits for a signature once the server accepted the key; when the user
            // cancels, send an invalid one so the server rejects this attempt and
            // authentication can continue with other methods.
            let blob = match (signed, public) {
                (Some(sig), _) => sig,
                (None, Some(public)) => dummy_signature(&public, hash_alg),
                (None, None) => Vec::new(),
            };
            blob.encode(&mut to_sign)
                .map_err(|e| SignerError(e.to_string()))?;
            Ok(to_sign)
        }
    }
}

/// Wire-encoded signature of `data`.
fn sign(
    key: &PrivateKey,
    hash_alg: Option<HashAlg>,
    data: &[u8],
) -> std::result::Result<Vec<u8>, String> {
    use russh::keys::signature::Signer;
    let signature: Signature = match key.key_data() {
        KeypairData::Rsa(rsa) => Signer::try_sign(&(rsa, hash_alg), data),
        other => Signer::try_sign(other, data),
    }
    .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    signature.encode(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn dummy_signature(key: &PublicKey, hash_alg: Option<HashAlg>) -> Vec<u8> {
    let alg = if key.algorithm().is_rsa() {
        match hash_alg {
            Some(HashAlg::Sha512) => "rsa-sha2-512".to_string(),
            Some(HashAlg::Sha256) => "rsa-sha2-256".to_string(),
            _ => "ssh-rsa".to_string(),
        }
    } else {
        key.algorithm().to_string()
    };
    let mut out = Vec::new();
    let _ = alg.as_str().encode(&mut out);
    let _ = [0u8; 0].as_slice().encode(&mut out);
    out
}
