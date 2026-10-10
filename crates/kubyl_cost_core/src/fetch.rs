//! Reading OpenCost: allocation requests through a [`Transport`] and errors that say what to
//! do about them.

use kubyl_metrics_core::transport::{PromError, Transport};
use serde_json::Value;

use crate::model::{self, Allocation, ParseError};
use crate::settings::Endpoint;
use crate::window::{ALLOCATION_PATH, Query};

/// What an allocation read can end in, as a message for the user (not an empty table).
#[derive(Clone, Debug, PartialEq)]
pub enum CostError {
    /// The Service answers nothing useful: where it is and what to check.
    Unreachable(String),
    /// OpenCost answered but can't compute (Prometheus down, too little data).
    OpenCost(String),
}

impl std::fmt::Display for CostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CostError::Unreachable(m) | CostError::OpenCost(m) => f.write_str(m),
        }
    }
}

/// A message for a failed request to the Service at `endpoint`.
pub fn describe(error: &PromError, endpoint: &Endpoint) -> CostError {
    let svc = format!("{}/{}", endpoint.namespace, endpoint.service);
    CostError::Unreachable(match error {
        PromError::Http(403, _) => format!(
            "Not allowed to reach {svc}: the API server's service proxy needs the get verb on services/proxy in {}.",
            endpoint.namespace
        ),
        PromError::Http(404, _) => format!(
            "{svc} doesn't answer on port {}: the Service or the port isn't there (settings: cost.clusters.<cluster>.service).",
            endpoint.port
        ),
        PromError::Http(503, _) => format!("{svc} has no ready pods (503). Is OpenCost running?"),
        PromError::Http(400, _) => {
            "OpenCost refused the query (400): is this OpenCost's API?".to_string()
        }
        PromError::Http(500..=599, message) if !message.is_empty() => {
            format!("OpenCost failed: {message}")
        }
        PromError::Timeout => format!(
            "{svc} didn't answer in 30 s: port {} may be wrong (the API is 9003), or OpenCost is still computing a long window (try 24 hours).",
            endpoint.port
        ),
        other => format!("Couldn't read {svc}: {other}"),
    })
}

/// The allocations of `query`: one set when accumulated, else one per step.
pub async fn allocations(
    transport: &Transport,
    endpoint: &Endpoint,
    query: &Query,
) -> Result<Vec<Vec<Allocation>>, CostError> {
    let params = query.params();
    let response: Value = transport
        .get(ALLOCATION_PATH, &params)
        .await
        .map_err(|e| describe(&e, endpoint))?;
    parse(&response)
}

/// A response body as allocations, errors as messages.
pub fn parse(response: &Value) -> Result<Vec<Vec<Allocation>>, CostError> {
    model::parse(response).map_err(|e| match e {
        ParseError::Api(message) => CostError::OpenCost(describe_api_message(&message)),
        ParseError::Invalid(why) => CostError::Unreachable(format!(
            "The Service answers, but not with OpenCost's allocation API ({why}): is {ALLOCATION_PATH} on this port?"
        )),
    })
}

/// OpenCost's own words, with the usual cause added.
fn describe_api_message(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("prometheus") || lower.contains("connection refused") {
        format!(
            "OpenCost can't read Prometheus: {message}. OpenCost needs the Prometheus it was installed with."
        )
    } else {
        format!("OpenCost says: {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn endpoint() -> Endpoint {
        "opencost/opencost:9003".parse().unwrap()
    }

    #[test]
    fn failed_requests_say_what_to_check() {
        let say = |e: PromError| describe(&e, &endpoint()).to_string();
        assert!(
            say(PromError::Http(403, String::new()))
                .contains("get verb on services/proxy in opencost")
        );
        assert!(say(PromError::Http(404, String::new())).contains("cost.clusters"));
        assert!(say(PromError::Http(503, String::new())).contains("no ready pods"));
        assert!(say(PromError::Http(400, String::new())).contains("refused the query"));
        assert!(say(PromError::Http(502, "bad gateway".into())).contains("bad gateway"));
        assert!(
            say(PromError::Timeout).contains("try 24 hours")
                && say(PromError::Timeout).contains("9003")
        );
        assert!(say(PromError::Transport("dns".into())).contains("opencost/opencost"));
    }

    #[test]
    fn opencost_errors_name_prometheus() {
        let err = parse(
            &json!({"code": 500, "message": "error querying Prometheus: connection refused"}),
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("can't read Prometheus") && text.contains("needs the Prometheus"),
            "{text}"
        );
        assert!(matches!(err, CostError::OpenCost(_)));
        let other = parse(&json!({"code": 400, "message": "bad window"}))
            .unwrap_err()
            .to_string();
        assert!(other.contains("OpenCost says: bad window"));
        let invalid = parse(&json!("<html>")).unwrap_err().to_string();
        assert!(invalid.contains("allocation API"), "{invalid}");
        assert!(parse(&json!({"code": 200, "data": []})).unwrap().is_empty());
    }
}
