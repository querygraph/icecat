use crate::error;
use grust_cypher::{
    ast::{BinaryOp, Clause, Expr, Projection},
    parser::parse_query,
};
use grustcat::grust::{GrustError, Result};
use std::collections::{HashMap, HashSet};

/// Immutable validated physical plan. Private fields prevent bypassing admission.
#[derive(Debug)]
pub struct PreparedQuery {
    pub(crate) algorithm: String,
    pub(crate) source: Option<Expr>,
    pub(crate) columns: Vec<(usize, String)>,
}

fn unsupported() -> GrustError {
    GrustError::Unsupported("grustcat-cypher admits one algorithm CALL, explicit YIELD, and RETURN; fullPaths requires UNWIND range(0,size(nodeIds)-1) and count/sum aggregates".into())
}
fn function<'a>(expr: &'a Expr, name: &str) -> Option<&'a [Expr]> {
    match expr {
        Expr::Function {
            name: n,
            distinct: false,
            star: false,
            args,
        } if n.eq_ignore_ascii_case(name) => Some(args),
        _ => None,
    }
}
fn output_name(expr: &Expr, alias: &Option<String>) -> Result<String> {
    match (alias, expr) {
        (Some(name), _) => Ok(name.clone()),
        (None, Expr::Variable(name)) => Ok(name.clone()),
        _ => Err(error("aggregate RETURN items require aliases")),
    }
}
fn projection(p: &Projection) -> Result<()> {
    if p.star
        || p.distinct
        || !p.order_by.is_empty()
        || p.skip.is_some()
        || p.limit.is_some()
        || p.items.is_empty()
    {
        return Err(unsupported());
    }
    Ok(())
}

impl PreparedQuery {
    /// Parse with Grust and admit the entire query AST, including argument,
    /// binding, projection and aggregate checks. No substring dispatch.
    pub fn parse(text: &str) -> Result<Self> {
        let query = parse_query(text).map_err(|e| GrustError::CypherSyntax(format!("{e:?}")))?;
        grust_cypher::semantics::analyze(&query)?;
        if query.parts.len() != 1 || query.parts[0].union.is_some() {
            return Err(unsupported());
        }
        let clauses = &query.parts[0].query.clauses;
        let Some(Clause::Call(call)) = clauses.first() else {
            return Err(unsupported());
        };
        let name = call.name.to_ascii_lowercase();
        let algorithm = name
            .strip_prefix("grustcat.")
            .ok_or_else(unsupported)?
            .to_string();
        let (fields, has_source): (&[&str], bool) = match algorithm.as_str() {
            "bfs" | "dijkstra" => (&["node_id", "distance"], true),
            "wcc" | "scc" => (&["node_id", "component_id"], false),
            "pagerank" => (&["node_id", "score"], false),
            "fullpaths" => (&["nodeIds", "costs"], true),
            _ => return Err(unsupported()),
        };
        if call.where_clause.is_some()
            || call.args.len() != usize::from(has_source)
            || call.yields.is_empty()
        {
            return Err(unsupported());
        }
        let source = if has_source {
            match &call.args[0] {
                Expr::Integer(_) | Expr::Parameter(_) => Some(call.args[0].clone()),
                _ => return Err(error("source must be an integer literal or parameter")),
            }
        } else {
            None
        };
        let mut bindings = HashMap::new();
        for (field, alias) in &call.yields {
            let index = fields
                .iter()
                .position(|f| *f == field)
                .ok_or_else(|| error(format!("unknown YIELD field {field}")))?;
            let name = alias.as_ref().unwrap_or(field).clone();
            if bindings.insert(name, index).is_some() {
                return Err(error("duplicate YIELD binding"));
            }
        }
        let mut columns = Vec::new();
        if algorithm == "fullpaths" {
            let [_, Clause::Unwind(unwind), Clause::Return(ret)] = clauses.as_slice() else {
                return Err(unsupported());
            };
            if bindings.contains_key(&unwind.alias) {
                return Err(error("UNWIND binding shadows YIELD"));
            }
            let args = function(&unwind.expr, "range").ok_or_else(unsupported)?;
            let [
                Expr::Integer(0),
                Expr::Binary {
                    op: BinaryOp::Subtract,
                    lhs,
                    rhs,
                },
            ] = args
            else {
                return Err(unsupported());
            };
            if **rhs != Expr::Integer(1) {
                return Err(unsupported());
            }
            let size = function(lhs, "size").ok_or_else(unsupported)?;
            let [Expr::Variable(nodes)] = size else {
                return Err(unsupported());
            };
            if bindings.get(nodes) != Some(&0) {
                return Err(unsupported());
            }
            projection(&ret.projection)?;
            for item in &ret.projection.items {
                let index = match &item.expr {
                    Expr::Function {
                        name,
                        distinct: false,
                        star: true,
                        args,
                    } if name.eq_ignore_ascii_case("count") && args.is_empty() => 0,
                    expr => {
                        let args = function(expr, "sum").ok_or_else(unsupported)?;
                        let [Expr::Index { base, index }] = args else {
                            return Err(unsupported());
                        };
                        let (Expr::Variable(array), Expr::Variable(i)) = (&**base, &**index) else {
                            return Err(unsupported());
                        };
                        if *i != unwind.alias {
                            return Err(error("unbound array index"));
                        }
                        1 + *bindings.get(array).ok_or_else(|| error("unbound array"))?
                    }
                };
                columns.push((index, output_name(&item.expr, &item.alias)?));
            }
        } else {
            let [_, Clause::Return(ret)] = clauses.as_slice() else {
                return Err(unsupported());
            };
            projection(&ret.projection)?;
            for item in &ret.projection.items {
                let Expr::Variable(name) = &item.expr else {
                    return Err(unsupported());
                };
                let index = *bindings
                    .get(name)
                    .ok_or_else(|| error(format!("unbound RETURN variable {name}")))?;
                columns.push((index, output_name(&item.expr, &item.alias)?));
            }
        }
        let mut names = HashSet::new();
        if columns.iter().any(|(_, name)| !names.insert(name.clone())) {
            return Err(error("duplicate output name"));
        }
        Ok(Self {
            algorithm,
            source,
            columns,
        })
    }
}
