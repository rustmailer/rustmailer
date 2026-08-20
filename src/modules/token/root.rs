// Copyright © 2025-2026 rustmailer.com
// Licensed under RustMailer License Agreement v1.0
// Unauthorized copying, modification, or distribution is prohibited.

use crate::{
    decrypt, encrypt, generate_token,
    modules::{
        error::{code::ErrorCode, RustMailerResult},
        settings::{dir::DATA_DIR_MANAGER, system::SystemSetting},
    },
    raise_error,
};
use std::fs::File;
use std::io::Write;

pub const ROOT_TOKEN: &str = "root-token";
pub const ROOT_PASSWORD: &str = "root-password";
pub const DEFAULT_ROOT_PASSWORD: &str = "root";
pub const ROOT_TOKEN_FILE: &str = "root";

async fn get_or_generate(
    key: &str,
    generate: impl Fn() -> String,
    save_file_name: Option<&str>,
    force: bool,
) -> RustMailerResult<String> {
    if let Some(existing_value) = SystemSetting::get_existing_value(key).await? {
        if force {
            // If force is true, write the existing value to the file
            if let Some(filename) = save_file_name {
                save_to_file(&existing_value.to_string(), filename).await?;
            }
        }
        Ok(existing_value)
    } else {
        // If no value exists, generate a new value
        let new_value = generate();
        SystemSetting::set_value(key, new_value.clone()).await?;

        // Write the new value to the file, if specified
        if let Some(filename) = save_file_name {
            save_to_file(&new_value.to_string(), filename).await?;
        }
        Ok(new_value)
    }
}

pub async fn ensure_root_token() -> RustMailerResult<()> {
    get_or_generate(
        ROOT_TOKEN,
        || generate_token!(128),
        Some(ROOT_TOKEN_FILE),
        true,
    )
    .await?;
    Ok(())
}

pub async fn reset_root_token() -> RustMailerResult<String> {
    let new_token = generate_token!(128);
    save_new_token(&new_token).await?;
    save_to_file(&new_token, ROOT_TOKEN_FILE).await?;
    Ok(new_token)
}

async fn save_new_token(token: &str) -> RustMailerResult<()> {
    let setting = SystemSetting::new(ROOT_TOKEN.to_string(), token.to_string());
    setting.set().await
}

async fn save_to_file(content: &str, filename: &str) -> RustMailerResult<()> {
    let file_path = DATA_DIR_MANAGER.root_dir.join(filename);
    let mut file = File::create(&file_path)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    writeln!(file, "{}", content)
        .map_err(|e| raise_error!(format!("{:#?}", e), ErrorCode::InternalError))?;
    Ok(())
}

pub async fn check_root_password(password: &str) -> RustMailerResult<String> {
    let stored_encrypted_password = SystemSetting::get_existing_value(ROOT_PASSWORD).await?;
    let matched = match stored_encrypted_password {
        Some(ref stored) => {
            let decrypted = decrypt!(stored)?;
            decrypted == password
        }
        None => DEFAULT_ROOT_PASSWORD == password,
    };

    if !matched {
        return Err(raise_error!(
            "Invalid password".into(),
            ErrorCode::PermissionDenied
        ));
    }

    let root_token = SystemSetting::get_existing_value(ROOT_TOKEN)
        .await?
        .ok_or_else(|| {
            raise_error!(
                "Root token not found — this should never happen".into(),
                ErrorCode::InternalError
            )
        })?;

    Ok(root_token)
}

pub async fn set_root_password(new_password: &str) -> RustMailerResult<()> {
    let encrypted_password = encrypt!(new_password)?;
    SystemSetting::set_value(ROOT_PASSWORD, encrypted_password).await
}
