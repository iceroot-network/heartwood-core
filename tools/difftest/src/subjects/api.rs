//! The API's query grammar and pagination: `api.parseQuery` (a query string to the parsed object)
//! and `api.paginate` (`{path, query, total}` to the list's `meta`), answered by the route
//! corpus's harness ([`crate::api`]).

use super::{Env, Outcome, Run};
use crate::subject::{Case, Verdict};

/// The operations of the API.
pub(super) const OPS: &[(&str, Run)] =
    &[("api.parseQuery", parse_query), ("api.paginate", paginate)];

fn parse_query(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = env;
    crate::api::parse_query_case(case)
}

fn paginate(env: &Env<'_>, case: &Case<'_>) -> Outcome<Verdict> {
    let _ = env;
    crate::api::paginate_case(case)
}
