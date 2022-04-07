use anyhow::Result;
use spin_sdk::{
    http::{Request, Response},
    http_component,
};
use matchit::Router;

/// A simple Spin HTTP component.
#[http_component]
fn hello_world(req: Request) -> Result<Response> {
    let mut router = Router::new();

    router.insert("/home", handler_fn(home)).unwrap();
    router.insert("/x/:id", handler_fn(x)).unwrap();
    router.insert("/", handler_fn(root)).unwrap();

    match router.at(req.uri().path()) {
        Ok(m) => {
            let f = *m.value;
            let s = f(&m.params);
            Ok(http::Response::builder()
                .status(200)
                .header("foo", "bar")
                .body(Some((s).into()))?)
        },
        Err(_) => {
            Ok(http::Response::builder()
                .status(404)
                .body(None)?
            )
        }
    }
}

fn handler_fn(f: fn(&matchit::Params) -> String) -> fn(&matchit::Params) -> String {
    f
}

fn home(_: &matchit::Params) -> String {
    "Home!!!".to_owned()
}

fn root(_: &matchit::Params) -> String {
    "ROOTSVILLE".to_owned()
}

fn x(params: &matchit::Params) -> String {
    match params.get("id") {
        None => "WHO?!?!?!?!".to_owned(),
        Some(id) => format!("HELLO {}", id),
    }
}

// struct Route {

// }

// impl Route {
//     pub fn handle<T>(m: matchit::Match<_, _, &T>) -> String {

//     }
// }
