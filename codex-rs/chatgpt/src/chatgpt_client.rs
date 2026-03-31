use codex_core::config::Config;
use codex_core::default_client::create_client;

use crate::chatgpt_token::get_chatgpt_token_data;
use crate::chatgpt_token::init_chatgpt_token_from_auth;

use anyhow::Context;
use serde::de::DeserializeOwned;
use std::time::Duration;

/// Make a GET request to the ChatGPT backend API.
pub(crate) async fn chatgpt_get_request<T: DeserializeOwned>(
    config: &Config,
    path: String,
) -> anyhow::Result<T> {
    chatgpt_get_request_with_timeout(config, path, /*timeout*/ None).await
}

//
// 编号（如：1）：修改
// 主要修改内容：将 ChatGPT 账户缺失时的重新登录提示命令改为 ikuncodex login。
// 修改目的：保证用户遇到账户态问题时，错误提示里的恢复步骤与新 CLI 命令一致。
//

pub(crate) async fn chatgpt_get_request_with_timeout<T: DeserializeOwned>(
    config: &Config,
    path: String,
    timeout: Option<Duration>,
) -> anyhow::Result<T> {
    let chatgpt_base_url = &config.chatgpt_base_url;
    init_chatgpt_token_from_auth(&config.codex_home, config.cli_auth_credentials_store_mode)
        .await?;

    // Make direct HTTP request to ChatGPT backend API with the token
    let client = create_client();
    let url = format!("{chatgpt_base_url}{path}");

    let token =
        get_chatgpt_token_data().ok_or_else(|| anyhow::anyhow!("ChatGPT token not available"))?;

    let account_id = token.account_id.ok_or_else(|| {
        anyhow::anyhow!("ChatGPT account ID not available, please re-run `ikuncodex login`")
    });

    let mut request = client
        .get(&url)
        .bearer_auth(&token.access_token)
        .header("chatgpt-account-id", account_id?)
        .header("Content-Type", "application/json");

    if let Some(timeout) = timeout {
        request = request.timeout(timeout);
    }

    let response = request.send().await.context("Failed to send request")?;

    if response.status().is_success() {
        let result: T = response
            .json()
            .await
            .context("Failed to parse JSON response")?;
        Ok(result)
    } else {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Request failed with status {status}: {body}")
    }
}
