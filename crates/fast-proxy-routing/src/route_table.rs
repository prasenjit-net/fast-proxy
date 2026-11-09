use std::collections::HashMap;

use fast_proxy_model::RoutePrefix;

/// A route found for a request path, plus the part of the path after the prefix.
#[derive(Debug)]
pub struct RouteMatch<'table, 'path, T> {
    pub route: &'table T,
    /// The path after the matched prefix: empty when the path equals the
    /// prefix, otherwise it starts with `/`. For the `/` route it is the
    /// whole path.
    pub remainder: &'path str,
}

/// Longest-prefix route lookup on path segment boundaries: `/api` matches
/// `/api` and `/api/x`, never `/apix`. The `/` route is the catch-all.
#[derive(Debug, Clone)]
pub struct RouteTable<T> {
    by_prefix: HashMap<String, T>,
}

impl<T> Default for RouteTable<T> {
    fn default() -> Self {
        Self {
            by_prefix: HashMap::new(),
        }
    }
}

impl<T> RouteTable<T> {
    /// Builds the table; a later route with the same prefix replaces an
    /// earlier one (the compiler rejects duplicates up front).
    pub fn build(routes: impl IntoIterator<Item = (RoutePrefix, T)>) -> Self {
        let by_prefix = routes
            .into_iter()
            .map(|(prefix, route)| (prefix.as_str().to_string(), route))
            .collect();
        Self { by_prefix }
    }

    pub fn len(&self) -> usize {
        self.by_prefix.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_prefix.is_empty()
    }

    /// Finds the route with the longest prefix that matches `path` (the
    /// path only, without a query string). Paths not starting with `/`
    /// match nothing.
    pub fn find<'table, 'path>(
        &'table self,
        path: &'path str,
    ) -> Option<RouteMatch<'table, 'path, T>> {
        if !path.starts_with('/') {
            return None;
        }
        let mut candidate = path;
        while candidate.len() > 1 {
            if let Some(route) = self.by_prefix.get(candidate) {
                return Some(RouteMatch {
                    route,
                    remainder: &path[candidate.len()..],
                });
            }
            // Drop the last segment; stops at "" once only "/" remains.
            candidate = &candidate[..candidate.rfind('/')?];
        }
        self.by_prefix.get("/").map(|route| RouteMatch {
            route,
            remainder: path,
        })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn table(prefixes: &[&str]) -> RouteTable<String> {
        RouteTable::build(
            prefixes
                .iter()
                .map(|p| (RoutePrefix::parse(p).unwrap(), (*p).to_string())),
        )
    }

    fn found(table: &RouteTable<String>, path: &str) -> Option<(String, String)> {
        table
            .find(path)
            .map(|m| (m.route.clone(), m.remainder.to_string()))
    }

    fn matched(prefix: &str, remainder: &str) -> Option<(String, String)> {
        Some((prefix.to_string(), remainder.to_string()))
    }

    #[test]
    fn longest_prefix_wins() {
        let table = table(&["/", "/api", "/api/v1"]);
        assert_eq!(found(&table, "/api/v1/users"), matched("/api/v1", "/users"));
        assert_eq!(found(&table, "/api/v2"), matched("/api", "/v2"));
        assert_eq!(found(&table, "/other"), matched("/", "/other"));
    }

    #[test]
    fn matches_only_on_segment_boundaries() {
        let table = table(&["/api"]);
        assert_eq!(found(&table, "/api"), matched("/api", ""));
        assert_eq!(found(&table, "/api/"), matched("/api", "/"));
        assert_eq!(found(&table, "/api/x"), matched("/api", "/x"));
        assert_eq!(found(&table, "/apix"), None);
        assert_eq!(found(&table, "/ap"), None);
    }

    #[test]
    fn the_root_route_is_the_catch_all_and_keeps_the_whole_path() {
        let table = table(&["/"]);
        assert_eq!(found(&table, "/"), matched("/", "/"));
        assert_eq!(found(&table, "/a/b"), matched("/", "/a/b"));
    }

    #[test]
    fn no_root_route_means_unmatched_paths_find_nothing() {
        let table = table(&["/api"]);
        assert_eq!(found(&table, "/"), None);
        assert_eq!(found(&table, "/other"), None);
    }

    #[test]
    fn paths_not_starting_with_a_slash_match_nothing() {
        let table = table(&["/"]);
        assert_eq!(found(&table, "*"), None);
        assert_eq!(found(&table, ""), None);
    }

    #[test]
    fn encoded_and_dotted_paths_are_matched_literally() {
        let table = table(&["/", "/a"]);
        assert_eq!(found(&table, "/a%2Fb"), matched("/", "/a%2Fb"));
        assert_eq!(found(&table, "/a/../b"), matched("/a", "/../b"));
        assert_eq!(found(&table, "//a"), matched("/", "//a"));
    }

    #[test]
    fn reports_size() {
        assert!(RouteTable::<String>::default().is_empty());
        assert_eq!(table(&["/", "/a"]).len(), 2);
    }

    /// Reference implementation: scan every prefix, keep the longest that
    /// matches on a segment boundary.
    fn brute_force<'a>(prefixes: &'a [String], path: &str) -> Option<&'a str> {
        prefixes
            .iter()
            .filter(|p| {
                let p = p.as_str();
                p == "/" || path == p || (path.starts_with(p) && path[p.len()..].starts_with('/'))
            })
            .map(String::as_str)
            .max_by_key(|p| p.len())
    }

    fn segment() -> impl Strategy<Value = String> {
        proptest::sample::select(vec!["a", "b", "ab", "api", "v1"]).prop_map(String::from)
    }

    fn path_strategy() -> impl Strategy<Value = String> {
        (proptest::collection::vec(segment(), 0..5), any::<bool>()).prop_map(|(segs, slash)| {
            let mut path = format!("/{}", segs.join("/"));
            if slash && !segs.is_empty() {
                path.push('/');
            }
            path
        })
    }

    proptest! {
        #[test]
        fn find_equals_the_brute_force_reference(
            prefix_paths in proptest::collection::hash_set(path_strategy(), 0..8),
            path in path_strategy(),
        ) {
            let prefixes: Vec<String> = prefix_paths
                .iter()
                .map(|p| RoutePrefix::parse(p).unwrap().as_str().to_string())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let table = table(&prefixes.iter().map(String::as_str).collect::<Vec<_>>());
            let expected = brute_force(&prefixes, &path);
            let actual = table.find(&path);
            prop_assert_eq!(actual.as_ref().map(|m| m.route.as_str()), expected);
            if let Some(m) = actual {
                let prefix = m.route.as_str();
                let rebuilt = if prefix == "/" { m.remainder.to_string() } else { format!("{prefix}{}", m.remainder) };
                prop_assert_eq!(rebuilt, path);
            }
        }
    }
}
