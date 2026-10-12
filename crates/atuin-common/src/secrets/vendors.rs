//! Credential formats of particular services, mostly from the rules of
//! [gitleaks](https://github.com/gitleaks/gitleaks/blob/master/config/gitleaks.toml) (MIT), where
//! a format has a prefix of its own (`sk-ant-`, `glpat-`, ...) and so can't be mistaken for
//! anything else; and of a few services gitleaks has no rule for, from their own documentation.
//!
//! Like the patterns in the parent module, these decide what history keeps
//! (`secrets_filter`), not only what is redacted, so a new one needs a prefix that real text
//! doesn't begin with.
//!
//! Every test value is a made-up credential of the right shape, split with `concat!` so this
//! file holds no contiguous token GitHub push protection would take for a real one.

use super::Pattern;
#[cfg(test)]
use super::REDACTED;
#[cfg(test)]
use super::tests::Test;

/// The credential formats of particular services.
pub(super) static VENDOR_PATTERNS: &[Pattern] = &[
    // gitleaks anthropic-api-key, anthropic-admin-api-key
    Pattern {
        name: "Anthropic API key",
        regex: r"(?<secret>sk-ant-(?:api|admin|oat|ort)[0-9]{2}-[A-Za-z0-9_-]{60,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("sk-ant-", "api03-3B51gfhrUARY3nyFt9x7rVcfHh9Twq8FEHlqILUEE2C8mhHWGW7N6_yW8wbV7XpfvqFCWf2Fpeqb61yImVhwzL0z02vZmAA"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("sk-ant-", "oat01-uawRmzL3Bp2AM5TUeuUQP3Rh4uUX9TheDuN1GRyqTWMWR83Zm6WgxpRpngWdhUHRwMF8H7qqlHMJODw6AIYFFt3jCCpAaX5"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks openai-api-key
    Pattern {
        name: "OpenAI API key",
        regex: r"(?<secret>sk-(?:proj|svcacct|admin)-[A-Za-z0-9_-]{20,}T3BlbkFJ[A-Za-z0-9_-]{20,}|sk-(?:proj|svcacct|admin)-[A-Za-z0-9_-]{60,}|sk-[A-Za-z0-9]{20}T3BlbkFJ[A-Za-z0-9]{20})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("sk-", "proj-RL1KFOza2IgZ8DTuINYnwUtShkSsOZPzXCWmXsgEXSwNDyHKL7jnpcW5AOT3BlbkFJsh6O1Mnq4VM5DGHXpqhgF8hJr9flWWlfuFLIPG0rMl1Ulks1EZTMWP9BLK"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("sk-", "ZwRP8JaUlieNfgx28h0pT3BlbkFJMAiLDq2nHaVoQELwrSP0"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("sk-", "svcacct-6MasLXI2iQhl2_3y4OkgXv4uZQE2a0Byr7aedknDnTxzj4_XaT2HdDpKYsufHCGz0sgx-Cfa23wL0IwH_L8M7sW0JipMXVsl_NUmWKeE8CUklw5ynKn2aMxB"),
                redacted: REDACTED,
            },
        ],
    },
    // OpenRouter's documented key format
    Pattern {
        name: "OpenRouter API key",
        regex: r"(?<secret>sk-or-v1-[a-f0-9]{64})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("sk-or-", "v1-2d181fe8bf2d2a204aacfada671ef04c0b9b91da55e8e71c006ac73ebaad35da"),
                redacted: REDACTED,
            },
        ],
    },
    // Groq's documented key format
    Pattern {
        name: "Groq API key",
        regex: r"(?<secret>gsk_[A-Za-z0-9]{52})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("gsk_", "qvRTVjhlBwDSiofDC0TnewFeFiWbvvx9fWSNHVUhrpUd7C2pIZhQ"),
                redacted: REDACTED,
            },
        ],
    },
    // xAI's documented key format
    Pattern {
        name: "xAI API key",
        regex: r"(?<secret>xai-[A-Za-z0-9]{60,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("xai-", "VIJsOWIWe4FOyg0UBbhuou8lzOBndtt5tWl8x6f6NebeHzmXApp6JhkgwdQvfx7cuxEE6XbIJHc3Vfbh"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks perplexity-api-key
    Pattern {
        name: "Perplexity API key",
        regex: r"(?<secret>pplx-[A-Za-z0-9]{48})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("pplx-", "2tXJBgxkuLnAtDECNI60kxK68sfoDuhO7QMFVYnF2guHxaiK"),
                redacted: REDACTED,
            },
        ],
    },
    // Pinecone's documented key format
    Pattern {
        name: "Pinecone API key",
        regex: r"(?<secret>pcsk_[A-Za-z0-9_]{50,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("pcsk_", "tIQ34wPFMVh4hgTULe0GpEnbKwxGdSYD8sL7pq0mvHIk6Q42vZtYSOa3V5CiHFXf6aFshL"),
                redacted: REDACTED,
            },
        ],
    },
    // LangSmith's documented key format
    Pattern {
        name: "LangSmith API key",
        regex: r"(?<secret>lsv2_(?:pt|sk)_[a-f0-9]{32}_[a-f0-9]{10})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("lsv2_", "pt_09bd6fc3217b72dbe8d21d988d22280b_56c2370277"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks gcp-api-key
    Pattern {
        name: "Google API key",
        regex: r"(?<secret>AIza[0-9A-Za-z_-]{35})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("AIza", "NuuiKsCW-pCl-Pscx5XxesvgIbsImncYUwu"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks huggingface-access-token, huggingface-organization-api-token
    Pattern {
        name: "Hugging Face token",
        regex: r"(?<secret>hf_[A-Za-z]{34}|api_org_[A-Za-z]{34})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("hf_", "uJYJVQlvPpxxKpalfUflUjMGgdjvzTwmSP"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("api_org_", "wscVsqvMPUqYxTKawGdgjDmOpMtVFmGZay"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks digitalocean-pat, -access-token, -refresh-token
    Pattern {
        name: "DigitalOcean token",
        regex: r"(?<secret>do[opr]_v1_[a-f0-9]{64})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("dop_", "v1_aee3f3a11aa5abfd9d3c5d14dad70bfe8739620559acab3253643910438854ac"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks sendgrid-api-token
    Pattern {
        name: "SendGrid API key",
        regex: r"(?<secret>SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("SG.", "38XFZU7fu4Kjm0408GQvom.5W1D2Wqzz1z4sZPDfGhQv5K3ILdfaPK3PlYL43nMgZA"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks sendinblue-api-token
    Pattern {
        name: "Brevo (Sendinblue) API key",
        regex: r"(?<secret>xkeysib-[a-f0-9]{64}-[A-Za-z0-9]{16})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("xkeysib-", "c87bba7769192bbcc846de9a6bb39caeaf430d80d4d944bc255f247fc430d0b0-ZoPPrarpKsQOhmIR"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks linear-api-key
    Pattern {
        name: "Linear API key",
        regex: r"(?<secret>lin_api_[A-Za-z0-9]{40})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("lin_api_", "T0V46WSSxBDmOVXMJyYVXQ4HLW7vH5JPUtX2EQaJ"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks doppler-api-token
    Pattern {
        name: "Doppler token",
        regex: r"(?<secret>dp\.(?:pt|st|ct|sa|scim|audit)\.[A-Za-z0-9]{40,44})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("dp.", "pt.C2cBiLC56nPm3KcAip85V9fci5vhqEQJDynta5vohbz"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks shopify-*
    Pattern {
        name: "Shopify token",
        regex: r"(?<secret>shp(?:at|ca|pa|ss)_[a-fA-F0-9]{32})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("shpat_", "acffe3344d9e39f2dd113dc0cfc9f02b"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks databricks-api-token
    Pattern {
        name: "Databricks token",
        regex: r"(?<secret>dapi[a-f0-9]{32}(?:-[0-9])?)",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("dapi", "423b74fd91df3276823d0ed0a46b66f0"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks pypi-upload-token
    Pattern {
        name: "PyPI upload token",
        regex: r"(?<secret>pypi-AgEIcHlwaS5vcmc[A-Za-z0-9_-]{50,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("pypi-", "AgEIcHlwaS5vcmcRUbocYZ3exkTRPwyPRlF_c_Cv4NF33HD0ZyLl3f70HMfSeKiUKWOJWr2oqO04FXLiq8p6tenNC9xptJZ"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks rubygems-api-token
    Pattern {
        name: "RubyGems API key",
        regex: r"(?<secret>rubygems_[a-f0-9]{48})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("rubygems_", "53cc2b547eb3f5bb0feb81453ba7d0f60ff6fc5f440031f4"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks notion-api-token
    Pattern {
        name: "Notion token",
        regex: r"(?<secret>ntn_[0-9]{11}[A-Za-z0-9]{35})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("ntn_", "24638176846w4J3TE0MyJ2JlxJ3nRj31D4Bka5jnYseWpe"),
                redacted: REDACTED,
            },
        ],
    },
    // Atlassian's documented token format
    Pattern {
        name: "Atlassian API token",
        regex: r"(?<secret>ATATT3[A-Za-z0-9_=-]{60,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("ATATT3", "cWlpL3ZfTLmsS09hGejxnkftMyDEHXjdQqIlvkFZFbCBCoGfI2ROyj4F5y368z3AUHZcwVQvh0DZtoWEvieHrzGCXERsSmbjSMsp6ysODh8hFb2AQiXdJwIrbWqfPAE4GjpFWEUepQPFRI68YS9QPBf9rS4ULZJcYsht68yDnp24Bw3ppcGmFHDn6q"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks age-secret-key
    Pattern {
        name: "age secret key",
        regex: r"(?<secret>AGE-SECRET-KEY-1[QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L]{58})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("AGE-SECRET-", "KEY-1X48AHA66E8U8T9MGSLEK920UUST906PXFMX9QREJGVR8RWL7AYQEH6YUSU"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks 1password-service-account-token
    Pattern {
        name: "1Password service account token",
        regex: r"(?<secret>ops_eyJ[A-Za-z0-9+/]{60,}={0,3})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("ops_", "eyJgiu5fD482QaQiKo/h1QsFGgyoFxPecQgdspp8TocdS4xcC/MEz/DO6+YbX5025V1369p5z2UArClniQoEGK4xqqdwcfjVy8sDs50bbpE1bDJOZetCaRsnSdKUakf1/ANvIRtN+Zj1zzZwagGX6zbqGHzLYjtVr72EZZsGyYbTp7pwHrowh131S/QlmA6TQ5jCFbj/U6uYyFYh0dIhm/kGxCb+v5QIyzOlkXbzQOwqBOuocnR1yMhjMq8fANfBeaAWC7m"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks slack-*
    Pattern {
        name: "Slack token",
        regex: r"(?<secret>xox[bpe](?:-[0-9]{10,13}){2,3}-[A-Za-z0-9-]{24,34}|xapp-[0-9]-[A-Z0-9]+-[0-9]+-[a-z0-9]+|xoxe(?:\.xox[bp])?-[0-9]-[A-Za-z0-9]{60,}|xox[ar]-(?:[0-9]-)?[0-9a-zA-Z]{8,48}|xox[os]-[0-9]+-[0-9]+-[0-9]+-[a-fA-F[0-9]]+)",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("xoxb-", "263704386311-9415197113976-LqMisGkclaYVDnzZMwo7pkSG"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("xapp-", "1-A1S7NN9WGQX-3013421030248-pk77qi9f1qa8ckx8dtwpgyrl0dzosqo205efln88s34kzakp50cwu60iokrebit1"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks gitlab-*
    Pattern {
        name: "GitLab token",
        regex: r"(?<secret>glpat-[A-Za-z0-9_-]{20,}(?:\.[0-9a-z]{9})?|glptt-[0-9a-f]{40}|glrt-[A-Za-z0-9_-]{20,}(?:\.[0-9a-z]{9})?|gl(?:dt|ft|soat|ffct)-[A-Za-z0-9_-]{20}|glcbt-[A-Za-z0-9]{1,5}_[A-Za-z0-9_-]{20}|glagent-[A-Za-z0-9_-]{50}|glimt-[A-Za-z0-9_-]{25}|gloas-[A-Za-z0-9_-]{64}|GR1348941[A-Za-z0-9_-]{20})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("glpat-", "MdwSCAqi5jtpCNvAIzwCTMAh8kN.2ozx6mveg"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("glrt-", "ffRGP9mc16jfJteSDZuI"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("gldt-", "m3EdPylFsIZuPcrRDFZP"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks grafana-*
    Pattern {
        name: "Grafana token",
        regex: r"(?<secret>glc_[A-Za-z0-9+/]{32,}={0,3}|glsa_[A-Za-z0-9]{32}_[A-Fa-f0-9]{8}|eyJrIjoi[A-Za-z0-9]{60,}={0,3})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("glsa_", "4BRM2cVKQLjDekxcvRsPHjJ2vuxpjp2A_5af7bd4d"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("glc_", "aThhALbknfhC1sUEi4mwbyHeIV8YofIVWAxePbK4"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks sentry-user-token, sentry-org-token
    Pattern {
        name: "Sentry token",
        regex: r"(?<secret>sntryu_[a-f0-9]{64}|sntrys_eyJ[A-Za-z0-9+/=_]{40,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("sntryu_", "b1f9bccc834ac13719995035c84362fe34a9c31fcb0ad51af617f10d122fba04"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks vault-service-token, vault-batch-token
    Pattern {
        name: "HashiCorp Vault token",
        regex: r"(?<secret>hvs\.[A-Za-z0-9_-]{60,}|hvb\.[A-Za-z0-9_-]{60,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("hvs.", "WIkfExSAvat6V4v4uTj9lBXaEijBVOFMuiIrSRPjZrheQxPQsONnUJZ3OhA58j4wTTarlEyBhJAu7XJ4Z7QO0SYzLWXRN1d"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks hashicorp-tf-api-token
    Pattern {
        name: "Terraform Cloud token",
        regex: r"(?<secret>[A-Za-z0-9]{14}\.atlasv1\.[A-Za-z0-9_=-]{60,70})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("5WCNGuTOl83yen.atlas", "v1.CKGmlgD5pV7L9wzGrZQyNOkYuXZ8k8ikHCDKczrBIvx2Na0C7gq8fMPL9uDrm6X7rWC"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks planetscale-*
    Pattern {
        name: "PlanetScale token",
        regex: r"(?<secret>pscale_(?:tkn|oauth|pw)_[A-Za-z0-9_=.-]{32,64})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("pscale_", "tkn_xJIB1AugE2HZzQeHOVUC98zTRg4l8r202t10XM95XHN"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks postman-api-token
    Pattern {
        name: "Postman API key",
        regex: r"(?<secret>PMAK-[a-fA-F0-9]{24}-[a-fA-F0-9]{34})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("PMAK-", "c271de63804731dce9f2a47b-f83134351731197d48898f884675a046db"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks readme-api-token
    Pattern {
        name: "ReadMe API key",
        regex: r"(?<secret>rdme_[a-z0-9]{70})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("rdme_", "ycjauupatyhs4v1waxb84ma9979016dd723lfotlfiphqr9co1srn6ba6wfpned6zoggss"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks prefect-api-token
    Pattern {
        name: "Prefect API key",
        regex: r"(?<secret>pnu_[A-Za-z0-9]{36})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("pnu_", "npvXA1O9iEhNMQ67KzQKajjN1o1RkcaW2ZPs"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks heroku-api-key-v2
    Pattern {
        name: "Heroku API key",
        regex: r"(?<secret>HRKU-AA[A-Za-z0-9_-]{58})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("HRKU-", "AAKviFRDTmBX3f6QWeT86xU3krZb84YiRbrxuztn5oHkMRK8mHnALAfxz5qv"),
                redacted: REDACTED,
            },
        ],
    },
    // Supabase's documented key formats
    Pattern {
        name: "Supabase secret key",
        regex: r"(?<secret>sb_secret_[A-Za-z0-9_-]{31,}|sbp_[a-f0-9]{40})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("sb_secret_", "QvektgaafqYYtZt4qGBTNvWf2vywUYG"),
                redacted: REDACTED,
            },
            Test {
                input: concat!("sbp_", "85bfa375c1d71925ab85db25c4323f20e359c9e2"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks artifactory-api-key
    Pattern {
        name: "Artifactory API key",
        regex: r"(?<secret>AKCp[A-Za-z0-9]{69})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("AKCp", "Fq1TakzzfwYQ7wQwsmJHmnuporDV2eCUdQSCcnSZLwCWKgqCoOh8EO2NqhrrGQa6AU9RU"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks aws-amazon-bedrock-api-key-long-lived
    Pattern {
        name: "Amazon Bedrock API key",
        regex: r"(?<secret>ABSK[A-Za-z0-9+/]{60,}={0,2})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("ABSK", "i3lvj6iwYA6IbJtkDEfhBZBBvRYAE38Iz4CydIKvYHGCQKvFScri8DptYbci0eh2YwuN2PRD0MXPrhMekVz1DtcaApGjQJtuC2GPRFb5IgCTyOxqX02hi7pKrma3523aVp98"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks easypost-api-token
    Pattern {
        name: "EasyPost API key",
        regex: r"(?<secret>EZ[AT]K[A-Za-z0-9]{54})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("EZAK", "ExqMi0yBeQ9y1f296NVrgmEjW5FXB7ZshK5DoRiK6vuLXgmfZp4u68"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks flyio-access-token
    Pattern {
        name: "Fly.io token",
        regex: r"(?<secret>fo1_[A-Za-z0-9_-]{43}|fm[12][ar]?_[A-Za-z0-9+/]{60,}={0,3})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("fo1_", "h5vbX8FgYECcuHKMbibewyhO1KePlDUr7oPEHB6Ve6L"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks infracost-api-token
    Pattern {
        name: "Infracost API key",
        regex: r"(?<secret>ico-[A-Za-z0-9]{32})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("ico-", "4pj8YwmQ1P8hhcyacWj8fByFWR92QCYX"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks dynatrace-api-token
    Pattern {
        name: "Dynatrace token",
        regex: r"(?<secret>dt0c01\.[A-Za-z0-9]{24}\.[A-Za-z0-9]{64})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("dt0c01.", "LL9rlVWTsxAp1yEal4jrtIsW.eDrOmlhDRUDZjIxZg9KwZgUcivD8xRIlPbA1gYI7yEhaEF8g7RHigsm7wUc6RNtD"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks frameio-api-token
    Pattern {
        name: "Frame.io token",
        regex: r"(?<secret>fio-u-[A-Za-z0-9_=-]{64})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("fio-u-", "zPyHxsa89BTIdwYT9kq7mI9WEV6fjyqhysJJFDP8kN3Cf6xoqhuhlzksIMbkWtXv"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks duffel-api-token
    Pattern {
        name: "Duffel API key",
        regex: r"(?<secret>duffel_(?:test|live)_[A-Za-z0-9_=-]{43})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("duffel_", "test_8D9dLjodoJ1NSKfy6wrodULoPYTOIqqoLUehe7K8DXO"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks clojars-api-token
    Pattern {
        name: "Clojars token",
        regex: r"(?<secret>CLOJARS_[a-z0-9]{60})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("CLOJARS_", "37ciyudqslofatzgv3dcykdl4prrsiy6fq5t0oczhha2mkpie69rbmzylppj"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks square-access-token
    Pattern {
        name: "Square token",
        regex: r"(?<secret>sq0atp-[A-Za-z0-9_-]{22,60}|sq0csp-[A-Za-z0-9_-]{43})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("sq0atp-", "XcUsJDJhcWy2liqUqEbqdp"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks stripe-access-token
    Pattern {
        name: "Stripe restricted key",
        regex: r"(?<secret>rk_(?:live|test)_[A-Za-z0-9]{24,}|sk_prod_[A-Za-z0-9]{24,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("rk_", "live_EU5L4p3ZlRgreygIOYFRE6ix"),
                redacted: REDACTED,
            },
        ],
    },
    // Docker's documented token format
    Pattern {
        name: "Docker Hub token",
        regex: r"(?<secret>dckr_pat_[A-Za-z0-9_-]{27})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("dckr_", "pat_pZeI1IESD5xIT1OKqhirzKDPDsg"),
                redacted: REDACTED,
            },
        ],
    },
    // Tailscale's documented key format
    Pattern {
        name: "Tailscale key",
        regex: r"(?<secret>tskey-(?:api|auth|client|scim|webhook)-[A-Za-z0-9]{8,}-[A-Za-z0-9]{20,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("tskey-", "auth-kf7KpW9W3d2CNTRL-fCipfMm6ToVUNO1homoxO4UXsSXKH5"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks cloudflare-origin-ca-key
    Pattern {
        name: "Cloudflare origin CA key",
        regex: r"(?<secret>v1\.0-[a-f0-9]{24}-[a-f0-9]{60,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("v1.0-", "186f84c3ac586e0a72bd8093-ec3ba54d51dae0be46a9db37552764784c26f73fedd30f83e8da9936c2da0b222ce27a42d6a38c2f19ea8585f07e2862c05cbe6fe3fa6c58437ad4dd7a711f6fa5d41e741ae0d9f331"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks azure-ad-client-secret
    Pattern {
        name: "Azure AD client secret",
        regex: r"(?<secret>[A-Za-z0-9_~.]{3}[0-9]Q~[A-Za-z0-9_~.-]{31,34})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("TN88Q~", "pSQEwdfDbLXP6zhxFZKL4hQHWvNVBRQqFX"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks telegram-bot-api-token
    Pattern {
        name: "Telegram bot token",
        regex: r"(?<secret>(?-u:\b)[0-9]{8,10}:AA[A-Za-z0-9_-]{33})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("1692872828:", "AAH59LIKCi8eGAOVdt4tn61OrleUzf5b6PV"),
                redacted: REDACTED,
            },
        ],
    },
    // Discord's documented webhook format
    Pattern {
        name: "Discord webhook",
        regex: r"(?<secret>https://(?:ptb\.|canary\.)?discord(?:app)?\.com/api/webhooks/[0-9]+/[A-Za-z0-9_-]{60,})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("https://discord.com/api/", "webhooks/634000504883015187/GytNCMs5aXiSXJkmtnVphQOkDBVOazMNTS8GUcJFqP1mamrDiMRnu4hwYxxJB7Sbaid9"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks microsoft-teams-webhook
    Pattern {
        name: "Microsoft Teams webhook",
        regex: r"(?<secret>https://[a-z0-9]+\.webhook\.office\.com/webhookb2/[a-z0-9@-]+/IncomingWebhook/[a-z0-9]{32}/[a-z0-9-]{36})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("https://acme.webhook.office.com/", "webhookb2/74d30252-5821-bd5d-ae48-6c7bdfb83014@b623fbbf-12dc-a9a9-3c55-87a71023b5f0/IncomingWebhook/eb872ac7ca5c59c26fb92de8af03a38e/1c4c417a-2d3f-781a-061f-12e543fbb8f8"),
                redacted: REDACTED,
            },
        ],
    },
    // gitleaks openshift-user-token
    Pattern {
        name: "OpenShift token",
        regex: r"(?<secret>sha256~[A-Za-z0-9_-]{43})",
        prefilter: None,
        #[cfg(test)]
        tests: &[
            Test {
                input: concat!("sha256~", "kRwNMI6B5Af2FBhgrTw5Sdf4VmGYMxrXcy9HDEoGwpv"),
                redacted: REDACTED,
            },
        ],
    },
];
