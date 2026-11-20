use nexssh_core::{Prompt, PromptReply, known_hosts::HostKeyStatus};
use slint::{Model, ModelRc, VecModel};

use crate::{AppWindow, PromptField};

pub fn show(ui: &AppWindow, prompt: &Prompt) {
    ui.set_prompt_auth(true);
    let field = |label: &str, secret: bool, value: &str| PromptField {
        label: label.into(),
        secret,
        value: value.into(),
    };
    let (title, detail, fields) = match prompt {
        Prompt::HostKey {
            host,
            port,
            key_type,
            fingerprint,
            check,
        } => {
            let warning = match check {
                HostKeyStatus::Changed {
                    known_fingerprint,
                    file,
                    line,
                } => format!(
                    "КЛЮЧ ИЗМЕНИЛСЯ. Возможна подмена сервера.\nПрежний: {known_fingerprint}\nЗапись: {file}:{line}\n\n"
                ),
                HostKeyStatus::Revoked => "Ключ отозван. Подключение запрещено.\n\n".into(),
                _ => "Проверьте отпечаток ключа перед подключением.\n\n".into(),
            };
            (
                "Ключ сервера".into(),
                format!("{warning}{host}:{port}\n{key_type}\n{fingerprint}"),
                vec![],
            )
        }
        Prompt::User {
            host, suggestion, ..
        } => (
            "Имя пользователя".into(),
            host.clone(),
            vec![field("Логин", false, suggestion)],
        ),
        Prompt::Password {
            user, host, error, ..
        } => (
            "Пароль SSH".into(),
            format!("{user}@{host}\n{}", error.as_deref().unwrap_or("")),
            vec![field("Пароль", true, "")],
        ),
        Prompt::Passphrase {
            key_path, error, ..
        } => (
            "Пароль ключа".into(),
            format!("{key_path}\n{}", error.as_deref().unwrap_or("")),
            vec![field("Парольная фраза", true, "")],
        ),
        Prompt::KeyboardInteractive {
            host,
            name,
            instructions,
            prompts,
        } => (
            "Авторизация SSH".into(),
            format!("{host}\n{name}\n{instructions}"),
            prompts
                .iter()
                .map(|p| field(&p.text, !p.echo, ""))
                .collect(),
        ),
    };
    ui.set_prompt_title(title);
    ui.set_prompt_detail(detail.into());
    ui.set_prompt_fields(ModelRc::new(VecModel::from(fields)));
    ui.set_prompt_host_key(matches!(prompt, Prompt::HostKey { .. }));
    ui.set_prompt_can_accept(!matches!(
        prompt,
        Prompt::HostKey {
            check: HostKeyStatus::Revoked,
            ..
        }
    ));
    ui.set_remember_host(false);
    ui.set_prompt_visible(true);
}

pub fn reply(ui: &AppWindow, prompt: &Prompt, accept: bool) -> PromptReply {
    if !accept {
        return PromptReply::Cancel;
    }
    let values: Vec<String> = ui
        .get_prompt_fields()
        .iter()
        .map(|f| f.value.to_string())
        .collect();
    match prompt {
        Prompt::HostKey { check, .. } => PromptReply::HostKey {
            accept: !matches!(check, HostKeyStatus::Revoked),
            remember: ui.get_remember_host(),
        },
        Prompt::User { .. } => PromptReply::User {
            name: values.first().cloned().unwrap_or_default(),
        },
        Prompt::KeyboardInteractive { .. } => PromptReply::Answers { values },
        _ => PromptReply::Secret {
            value: values.first().cloned().unwrap_or_default(),
            remember: false,
        },
    }
}

pub fn clear(ui: &AppWindow) {
    ui.set_prompt_accept_label("".into());
    ui.set_prompt_visible(false);
    ui.set_prompt_auth(false);
    ui.set_prompt_fields(ModelRc::default());
    ui.set_prompt_detail("".into());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revoked_keys_cannot_be_accepted_and_fields_are_cleared() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = AppWindow::new().unwrap();
        let prompt = Prompt::HostKey {
            host: "example.org".into(),
            port: 22,
            key_type: "ssh-ed25519".into(),
            fingerprint: "SHA256:test".into(),
            check: HostKeyStatus::Revoked,
        };
        show(&ui, &prompt);
        assert!(!ui.get_prompt_can_accept());
        assert!(matches!(
            reply(&ui, &prompt, true),
            PromptReply::HostKey { accept: false, .. }
        ));
        clear(&ui);
        assert!(!ui.get_prompt_visible());
        assert_eq!(ui.get_prompt_fields().row_count(), 0);
    }
}
