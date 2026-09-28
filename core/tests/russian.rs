//! Messages follow the configured language. This is a separate test binary because the
//! language is process-wide and the tests in `sshd.rs` expect English.

mod common;

use std::sync::Arc;

use common::{Session, core, env, server};
use nexssh_core::i18n::{self, Lang};
use nexssh_core::{AuthKind, Destination, Prompt, PromptReply, SessionStatus};

#[tokio::test(flavor = "multi_thread")]
async fn messages_are_translated() {
    i18n::set_language(Lang::Ru);

    // Validation errors (no server needed).
    let err = Destination::parse("").unwrap_err().to_string();
    assert_eq!(err, "Введите хост, например user@example.com:22");

    let Some(env) = env() else { return };
    let core = Arc::new(core());
    let saved = core
        .store
        .save_server(server(&env, AuthKind::Password))
        .unwrap();
    let mut s = Session::open(Arc::clone(&core), saved);
    s.accept_host_key().await;

    // A wrong password comes back with a translated hint.
    let (id, _) = s.expect_prompt().await;
    s.answer(
        id,
        PromptReply::Secret {
            value: "wrong".into(),
            remember: false,
        },
    );
    let (id, prompt) = s.expect_prompt().await;
    match prompt {
        Prompt::Password { error, .. } => {
            assert_eq!(
                error.as_deref(),
                Some("Доступ запрещён, попробуйте ещё раз.")
            );
        }
        other => panic!("expected a password prompt, got {other:?}"),
    }
    s.answer(
        id,
        PromptReply::Secret {
            value: env.password.clone(),
            remember: false,
        },
    );
    s.expect_status(SessionStatus::Connected).await;
    assert!(
        s.log.iter().any(|l| l.starts_with("Подключение к ")),
        "{:?}",
        s.log
    );
    assert!(
        s.log
            .iter()
            .any(|l| l == &format!("Вход выполнен как {} (пароль)", env.user)),
        "{:?}",
        s.log
    );

    s.write("exit 3\n");
    let (reason, failed) = s.expect_status_full(SessionStatus::Disconnected).await;
    assert_eq!(reason.as_deref(), Some("Сеанс завершён (код выхода 3)"));
    assert!(!failed);
    s.close().await;
}
