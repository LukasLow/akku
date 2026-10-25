//! Attribute macros for Akku.
//!
//! Users normally do not depend on this crate directly. `akku` re-exports the
//! macros under the HTTP domain:
//!
//! ```text
//! #[akku::http::get("/")]
//! #[akku::http::post("/users")]
//! #[akku::http::main]
//! ```

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, ItemFn, LitStr};

/// Mark an async function as a `GET` route.
///
/// The function becomes a generated route type with the same name. It can be
/// passed to `akku::http::App::mount`:
///
/// ```ignore
/// #[akku::http::get("/users/{id}")]
/// async fn user(Path(id): Path<u64>) -> Html {
///     Html::new(format!("<h1>User {id}</h1>"))
/// }
///
/// App::new().mount(user);
/// ```
#[proc_macro_attribute]
pub fn get(attr: TokenStream, item: TokenStream) -> TokenStream {
    route(attr, item, "get")
}

/// Mark an async function as a `POST` route.
///
/// The function becomes a generated route type with the same name, exactly like
/// [`get`].
#[proc_macro_attribute]
pub fn post(attr: TokenStream, item: TokenStream) -> TokenStream {
    route(attr, item, "post")
}

/// Run an async `main` function on Akku's Tokio runtime.
///
/// ```ignore
/// #[akku::http::main]
/// async fn main() -> akku::http::Result<()> {
///     App::new().serve("127.0.0.1:3000").await
/// }
/// ```
#[proc_macro_attribute]
pub fn main(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    if function.sig.asyncness.is_none() {
        return syn::Error::new_spanned(function.sig, "Akku main must be async")
            .to_compile_error()
            .into();
    }
    if !function.sig.inputs.is_empty() {
        return syn::Error::new_spanned(function.sig.inputs, "Akku main does not accept arguments")
            .to_compile_error()
            .into();
    }

    let attrs = function.attrs;
    let vis = function.vis;
    let mut signature = function.sig;
    signature.asyncness = None;
    let block = function.block;
    quote! {
        #(#attrs)*
        #vis #signature {
            ::akku::http::__private::run(async move #block)
        }
    }
    .into()
}

fn route(attr: TokenStream, item: TokenStream, method: &str) -> TokenStream {
    let path = parse_macro_input!(attr as LitStr);
    let mut function = parse_macro_input!(item as ItemFn);
    if function.sig.asyncness.is_none() {
        return syn::Error::new_spanned(function.sig, "Akku routes must be async")
            .to_compile_error()
            .into();
    }

    let name = function.sig.ident.clone();
    let handler = format_ident!("__akku_handler_{}", name);
    let visibility = function.vis.clone();
    let method = format_ident!("{}", method);

    function.sig.ident = handler.clone();
    function.vis = syn::Visibility::Inherited;

    quote! {
        #[allow(non_camel_case_types)]
        #[doc = "Generated route. Pass it to `akku::http::App::mount`."]
        #visibility struct #name;

        impl ::akku::http::Mount for #name {
            fn mount(
                self,
                router: ::akku::http::__private::Router,
            ) -> ::akku::http::__private::Router {
                router.route(#path, ::akku::http::__private::#method(#handler))
            }
        }

        #[doc(hidden)]
        #function
    }
    .into()
}
