//! The admin router is built through [`RouteTable`], which records every route
//! with its [`Access`]. That record is the single list a test walks to prove a
//! viewer cannot write, and a route cannot be registered without an access.

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::handler::Handler;
use axum::http::Method;
use axum::middleware::{from_fn_with_state, Next};
use axum::routing::{on, MethodFilter};
use axum::Router;
use fast_proxy_model::Action;

use crate::guard::{guard, Access};
use crate::state::SharedState;

/// The most a request carrying certificate material may weigh: a 256 KiB chain,
/// a 256 KiB key and JSON around them.
const KEY_BODY_LIMIT: usize = 600 * 1024;

/// One registered route. The router only needs the path and method; the list
/// exists for the tests that walk every route.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct RouteSpec {
    pub method: Method,
    pub path: &'static str,
    pub access: Access,
}

/// A router under construction and the routes it holds.
pub struct RouteTable {
    router: Router<SharedState>,
    specs: Vec<RouteSpec>,
    state: SharedState,
}

impl RouteTable {
    pub fn new(state: SharedState) -> Self {
        Self {
            router: Router::new(),
            specs: Vec::new(),
            state,
        }
    }

    pub fn get<H, T>(self, path: &'static str, access: Access, handler: H) -> Self
    where
        H: Handler<T, SharedState>,
        T: 'static,
    {
        self.add(Method::GET, MethodFilter::GET, path, access, handler)
    }

    pub fn post<H, T>(self, path: &'static str, access: Access, handler: H) -> Self
    where
        H: Handler<T, SharedState>,
        T: 'static,
    {
        self.add(Method::POST, MethodFilter::POST, path, access, handler)
    }

    pub fn put<H, T>(self, path: &'static str, access: Access, handler: H) -> Self
    where
        H: Handler<T, SharedState>,
        T: 'static,
    {
        self.add(Method::PUT, MethodFilter::PUT, path, access, handler)
    }

    pub fn delete<H, T>(self, path: &'static str, access: Access, handler: H) -> Self
    where
        H: Handler<T, SharedState>,
        T: 'static,
    {
        self.add(Method::DELETE, MethodFilter::DELETE, path, access, handler)
    }

    fn add<H, T>(
        mut self,
        method: Method,
        filter: MethodFilter,
        path: &'static str,
        access: Access,
        handler: H,
    ) -> Self
    where
        H: Handler<T, SharedState>,
        T: 'static,
    {
        let mut route = on(filter, handler);
        if access != Access::Open {
            route = route.layer(from_fn_with_state(
                self.state.clone(),
                move |State(state): State<SharedState>, request: Request, next: Next| {
                    guard(state, access, request, next)
                },
            ));
        }
        if access == Access::Needs(Action::Keys) {
            route = route.layer(DefaultBodyLimit::max(KEY_BODY_LIMIT));
        }
        self.router = self.router.route(path, route);
        self.specs.push(RouteSpec {
            method,
            path,
            access,
        });
        self
    }

    /// The router and the list of its routes.
    pub fn finish(self) -> (Router<SharedState>, Vec<RouteSpec>) {
        (self.router, self.specs)
    }
}
