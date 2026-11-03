pub(crate) fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

pub(crate) fn memory_namespace_predicate(scope: &str, visibility: &str, agent_id: &str) -> String {
    format!(
        "scope = '{}' AND visibility = '{}' AND agent_id = '{}'",
        escape_sql_literal(scope),
        escape_sql_literal(visibility),
        escape_sql_literal(agent_id),
    )
}

pub(crate) fn memory_row_predicate(scope: &str, visibility: &str, agent_id: &str, key: &str) -> String {
    format!(
        "{} AND key = '{}'",
        memory_namespace_predicate(scope, visibility, agent_id),
        escape_sql_literal(key),
    )
}
