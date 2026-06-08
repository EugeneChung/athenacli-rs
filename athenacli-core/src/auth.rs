//! Credential resolution + Athena client construction.
//!
//! Priority per field mirrors Python `config.AWSConfig`:
//!   CLI flag  >  `[aws_profile.<profile>]`  >  AWS default chain.
//! `region` additionally falls back to the SDK's default region chain.
//! `role_arn` is taken from the profile only (no CLI flag), as in Python.

use aws_config::sts::AssumeRoleProvider;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_credential_types::Credentials;
use aws_sdk_athena::Client;

use crate::config::AwsProfile;

/// Credentials/connection inputs from the CLI flags (each optional).
#[derive(Debug, Default, Clone)]
pub struct CliCreds {
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
    pub region: Option<String>,
    pub s3_staging_dir: Option<String>,
    pub work_group: Option<String>,
}

/// Resolved connection settings after applying the priority rules.
#[derive(Debug, Default, Clone)]
pub struct CredentialSpec {
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
    pub region: Option<String>,
    pub s3_staging_dir: Option<String>,
    pub work_group: Option<String>,
    pub role_arn: Option<String>,
    pub profile: String,
}

/// First non-empty value, mirroring Python `AWSConfig.get_val` truthiness.
fn pick<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

pub fn resolve(cli: &CliCreds, profile_name: &str, profile: Option<&AwsProfile>) -> CredentialSpec {
    let p = |f: fn(&AwsProfile) -> Option<&str>| profile.and_then(f);
    CredentialSpec {
        access_key_id: pick([
            cli.access_key_id.as_deref(),
            p(|x| x.aws_access_key_id.as_deref()),
        ]),
        secret_access_key: pick([
            cli.secret_access_key.as_deref(),
            p(|x| x.aws_secret_access_key.as_deref()),
        ]),
        session_token: pick([
            cli.session_token.as_deref(),
            p(|x| x.aws_session_token.as_deref()),
        ]),
        region: pick([cli.region.as_deref(), p(|x| x.region.as_deref())]),
        s3_staging_dir: pick([
            cli.s3_staging_dir.as_deref(),
            p(|x| x.s3_staging_dir.as_deref()),
        ]),
        work_group: pick([cli.work_group.as_deref(), p(|x| x.work_group.as_deref())]),
        role_arn: pick([p(|x| x.role_arn.as_deref())]),
        profile: profile_name.to_string(),
    }
}

/// Build an Athena client from a resolved spec. Returns the client and the
/// region the SDK actually resolved (for the prompt). Async — call once from
/// the owning runtime via `block_on`.
pub async fn build_client(spec: &CredentialSpec) -> anyhow::Result<(Client, Option<String>)> {
    let mut loader = aws_config::defaults(BehaviorVersion::latest()).profile_name(&spec.profile);

    if let Some(region) = &spec.region {
        loader = loader.region(Region::new(region.clone()));
    }
    if let (Some(ak), Some(sk)) = (&spec.access_key_id, &spec.secret_access_key) {
        let creds = Credentials::from_keys(ak, sk, spec.session_token.clone());
        loader = loader.credentials_provider(creds);
    }

    let base = loader.load().await;

    let sdk_config = if let Some(role_arn) = &spec.role_arn {
        let provider = AssumeRoleProvider::builder(role_arn)
            .session_name("athenacli")
            .configure(&base)
            .build()
            .await;
        base.to_builder()
            .credentials_provider(SharedCredentialsProvider::new(provider))
            .build()
    } else {
        base
    };

    let region = sdk_config.region().map(|r| r.as_ref().to_string());
    Ok((Client::new(&sdk_config), region))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile_with_keys() -> AwsProfile {
        AwsProfile {
            aws_access_key_id: Some("PROFILE_AK".into()),
            aws_secret_access_key: Some("PROFILE_SK".into()),
            aws_session_token: Some(String::new()),
            region: Some("ap-northeast-2".into()),
            s3_staging_dir: Some("s3://from-profile/".into()),
            work_group: Some(String::new()),
            role_arn: Some("arn:aws:iam::1:role/R".into()),
        }
    }

    #[test]
    fn cli_overrides_profile() {
        let cli = CliCreds {
            access_key_id: Some("CLI_AK".into()),
            region: Some("us-east-1".into()),
            ..Default::default()
        };
        let prof = profile_with_keys();
        let spec = resolve(&cli, "default", Some(&prof));
        assert_eq!(spec.access_key_id.as_deref(), Some("CLI_AK"));
        assert_eq!(spec.region.as_deref(), Some("us-east-1"));
        // not on CLI -> falls to profile
        assert_eq!(spec.secret_access_key.as_deref(), Some("PROFILE_SK"));
        assert_eq!(spec.s3_staging_dir.as_deref(), Some("s3://from-profile/"));
    }

    #[test]
    fn empty_strings_treated_as_unset() {
        let prof = profile_with_keys();
        let spec = resolve(&CliCreds::default(), "default", Some(&prof));
        // session_token / work_group are empty strings in the profile.
        assert_eq!(spec.session_token, None);
        assert_eq!(spec.work_group, None);
    }

    #[test]
    fn role_arn_comes_from_profile_only() {
        let prof = profile_with_keys();
        let spec = resolve(&CliCreds::default(), "default", Some(&prof));
        assert_eq!(spec.role_arn.as_deref(), Some("arn:aws:iam::1:role/R"));
        // No profile -> no role.
        let none = resolve(&CliCreds::default(), "default", None);
        assert_eq!(none.role_arn, None);
    }
}
