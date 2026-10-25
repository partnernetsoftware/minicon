//! Finite observations of existing framed upstream JSON; never changes its result.
use super::Protocol;
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum BillingDialect {
    #[default]
    None,
    OpenCode,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum InputSemantics {
    #[default]
    ChatInclusive,
    MessagesBasePlusCache,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TokenSnapshot {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub reported_total: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_create: Option<u64>,
    pub reasoning: Option<u64>,
    pub semantics: InputSemantics,
    pub invalid: bool,
    pub non_monotonic: bool,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ExactDecimal(String);
impl ExactDecimal {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty()
            || raw.len() > 128
            || !raw
                .bytes()
                .all(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E'))
        {
            return None;
        }
        // JSON's own grammar remains authoritative, including finite lexical
        // numbers too precise for f64. RawValue does not round their digits.
        let parsed: &RawValue = serde_json::from_str(raw).ok()?;
        if parsed.get().starts_with(['"', '{', '[']) || !raw.bytes().any(|c| c.is_ascii_digit()) {
            return None;
        }
        Some(Self(raw.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum BillingSource {
    Free,
    Balance,
    Go,
    GoPlus,
    Byok,
    Unknown,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct OpenCodeBilling {
    pub cost_usd: Option<ExactDecimal>,
    pub list_cost_usd: Option<ExactDecimal>,
    pub allowance_plan_usd: Option<ExactDecimal>,
    pub source: Option<BillingSource>,
    pub estimated: Option<bool>,
    pub billed_input: Option<u64>,
    pub billed_output: Option<u64>,
    pub reasoning: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write_5m: Option<u64>,
    pub cache_write_1h: Option<u64>,
    pub invalid: bool,
}
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct UsageSnapshot {
    pub tokens: TokenSnapshot,
    pub billing: Option<OpenCodeBilling>,
    pub text_seen: bool,
}
pub(crate) struct UsageObserver {
    protocol: Protocol,
    dialect: BillingDialect,
    snapshot: UsageSnapshot,
}
impl UsageObserver {
    pub fn new(protocol: Protocol) -> Self {
        let semantics = match protocol {
            Protocol::OpenAiChat => InputSemantics::ChatInclusive,
            Protocol::AnthropicMessages => InputSemantics::MessagesBasePlusCache,
        };
        Self {
            protocol,
            dialect: BillingDialect::None,
            snapshot: UsageSnapshot {
                tokens: TokenSnapshot {
                    semantics,
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }
    pub fn set_billing(&mut self, dialect: BillingDialect) {
        self.dialect = dialect;
    }
    pub fn snapshot(&self) -> UsageSnapshot {
        self.snapshot.clone()
    }
    pub fn observe_raw(&mut self, raw: &[u8]) {
        if self.dialect == BillingDialect::OpenCode {
            self.observe_billing(raw);
        }
    }
    pub fn observe_value(&mut self, value: &Value) {
        let usage = value.get("usage").filter(|v| !v.is_null()).or_else(|| {
            value
                .get("message")
                .and_then(|v| v.get("usage"))
                .filter(|v| !v.is_null())
        });
        if let Some(usage) = usage {
            if !usage.is_object() {
                self.snapshot.tokens.invalid = true;
            } else {
                self.observe_tokens(usage);
            }
        }
        self.snapshot.text_seen |= match self.protocol {
            Protocol::OpenAiChat => {
                value
                    .get("choices")
                    .and_then(Value::as_array)
                    .is_some_and(|choices| {
                        choices.iter().any(|choice| {
                            choice
                                .pointer("/delta/content")
                                .and_then(Value::as_str)
                                .is_some_and(|text| !text.is_empty())
                                || choice
                                    .pointer("/message/content")
                                    .and_then(Value::as_str)
                                    .is_some_and(|text| !text.is_empty())
                        })
                    })
            }
            Protocol::AnthropicMessages => {
                value
                    .pointer("/delta/text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
                    || value
                        .get("content")
                        .and_then(Value::as_array)
                        .is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .is_some_and(|text| !text.is_empty())
                            })
                        })
            }
        };
    }
    fn observe_tokens(&mut self, usage: &Value) {
        let t = &mut self.snapshot.tokens;
        let fields = match self.protocol {
            Protocol::OpenAiChat => [
                "prompt_tokens",
                "completion_tokens",
                "total_tokens",
                "",
                "",
                "",
            ],
            Protocol::AnthropicMessages => [
                "input_tokens",
                "output_tokens",
                "",
                "cache_read_input_tokens",
                "cache_creation_input_tokens",
                "",
            ],
        };
        for (name, target) in fields.into_iter().zip([
            &mut t.input,
            &mut t.output,
            &mut t.reported_total,
            &mut t.cache_read,
            &mut t.cache_create,
            &mut t.reasoning,
        ]) {
            if !name.is_empty() {
                update_counter(
                    usage.get(name),
                    target,
                    &mut t.invalid,
                    &mut t.non_monotonic,
                );
            }
        }
        if self.protocol == Protocol::OpenAiChat {
            update_counter(
                usage.pointer("/prompt_tokens_details/cached_tokens"),
                &mut t.cache_read,
                &mut t.invalid,
                &mut t.non_monotonic,
            );
            update_counter(
                usage.pointer("/completion_tokens_details/reasoning_tokens"),
                &mut t.reasoning,
                &mut t.invalid,
                &mut t.non_monotonic,
            );
        }
    }
    fn observe_billing(&mut self, raw: &[u8]) {
        let envelope: Result<Envelope<'_>, _> = serde_json::from_slice(raw);
        let Ok(envelope) = envelope else {
            self.snapshot.billing.get_or_insert_default().invalid = true;
            return;
        };
        let usage = envelope.usage.or_else(|| {
            envelope
                .message
                .and_then(|message| serde_json::from_str::<Envelope<'_>>(message.get()).ok())
                .and_then(|message| message.usage)
        });
        let Some(usage) = usage.filter(|usage| usage.get() != "null") else {
            return;
        };
        let Ok(usage) = serde_json::from_str::<RawUsage<'_>>(usage.get()) else {
            self.snapshot.billing.get_or_insert_default().invalid = true;
            return;
        };
        if usage.cost.is_none() && usage.opencode.is_none() {
            return;
        }
        let billing = self.snapshot.billing.get_or_insert_default();
        update_decimal(usage.cost, &mut billing.cost_usd, &mut billing.invalid);
        if let Some(opencode) = usage.opencode.filter(|value| value.get() != "null") {
            let Ok(fields) = serde_json::from_str::<RawBilling<'_>>(opencode.get()) else {
                billing.invalid = true;
                return;
            };
            update_decimal(
                fields.list_cost,
                &mut billing.list_cost_usd,
                &mut billing.invalid,
            );
            update_decimal(
                fields.allowance,
                &mut billing.allowance_plan_usd,
                &mut billing.invalid,
            );
            if let Some(source) = fields.billing_source {
                match serde_json::from_str::<&str>(source.get()) {
                    Ok(value) => {
                        billing.source = Some(match value {
                            "free" => BillingSource::Free,
                            "balance" => BillingSource::Balance,
                            "go" => BillingSource::Go,
                            "go-plus" => BillingSource::GoPlus,
                            "byok" => BillingSource::Byok,
                            _ => BillingSource::Unknown,
                        })
                    }
                    Err(_) => billing.invalid = true,
                }
            }
            if let Some(estimated) = fields.estimated {
                match serde_json::from_str::<bool>(estimated.get()) {
                    Ok(value) => billing.estimated = Some(value),
                    Err(_) => billing.invalid = true,
                }
            }
            if let Some(tokens) = fields.tokens {
                let Ok(tokens) = serde_json::from_str::<RawBilledTokens<'_>>(tokens.get()) else {
                    billing.invalid = true;
                    return;
                };
                for (value, target) in [
                    (tokens.input, &mut billing.billed_input),
                    (tokens.output, &mut billing.billed_output),
                    (tokens.reasoning, &mut billing.reasoning),
                    (tokens.cache_read, &mut billing.cache_read),
                    (tokens.cache_write_5m, &mut billing.cache_write_5m),
                    (tokens.cache_write_1h, &mut billing.cache_write_1h),
                ] {
                    if let Some(raw) = value {
                        match serde_json::from_str::<u64>(raw.get()) {
                            Ok(value) => *target = Some(value),
                            Err(_) => billing.invalid = true,
                        }
                    }
                }
            }
        }
    }
}
fn update_counter(
    value: Option<&Value>,
    target: &mut Option<u64>,
    invalid: &mut bool,
    decreased: &mut bool,
) {
    if let Some(value) = value {
        match value.as_u64() {
            Some(value) => {
                *decreased |= target.is_some_and(|old| value < old);
                *target = Some(value);
            }
            None => *invalid = true,
        }
    }
}
fn update_decimal(raw: Option<&RawValue>, target: &mut Option<ExactDecimal>, invalid: &mut bool) {
    if let Some(raw) = raw {
        match ExactDecimal::parse(raw.get()) {
            Some(value) => *target = Some(value),
            None => *invalid = true,
        }
    }
}
#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow)]
    usage: Option<&'a RawValue>,
    #[serde(borrow)]
    message: Option<&'a RawValue>,
}
#[derive(Deserialize)]
struct RawUsage<'a> {
    #[serde(borrow)]
    cost: Option<&'a RawValue>,
    #[serde(borrow)]
    opencode: Option<&'a RawValue>,
}
#[derive(Deserialize)]
struct RawBilling<'a> {
    #[serde(borrow)]
    list_cost: Option<&'a RawValue>,
    #[serde(borrow)]
    allowance: Option<&'a RawValue>,
    #[serde(borrow)]
    billing_source: Option<&'a RawValue>,
    #[serde(borrow)]
    estimated: Option<&'a RawValue>,
    #[serde(borrow)]
    tokens: Option<&'a RawValue>,
}
#[derive(Deserialize)]
struct RawBilledTokens<'a> {
    #[serde(borrow)]
    input: Option<&'a RawValue>,
    #[serde(borrow)]
    output: Option<&'a RawValue>,
    #[serde(borrow)]
    reasoning: Option<&'a RawValue>,
    #[serde(borrow)]
    cache_read: Option<&'a RawValue>,
    #[serde(borrow)]
    cache_write_5m: Option<&'a RawValue>,
    #[serde(borrow)]
    cache_write_1h: Option<&'a RawValue>,
}
