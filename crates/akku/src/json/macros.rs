//! The `json!` macro.
//!
//! The macro is exported at the crate root and re-exported inside
//! [`crate::json`] as `json!`, so callers write
//! `akku::json::json!({ "a": 1 })` and the crate root stays clean.

/// Build a [`crate::json::Value`] from a JSON-like literal.
///
/// Arrays and objects are written with the JSON syntax and may contain any
/// expression that implements [`crate::json::ToJson`]. Trailing commas are
/// allowed. Object keys must be string literals.
///
/// ```
/// use akku::json::json;
///
/// let name = "akku";
/// let value = json!({
///     "name": name,
///     "stars": 5,
///     "tags": ["rust", "json"],
///     "meta": { "ok": true, },
/// });
///
/// assert_eq!(value.get_path("meta.ok").and_then(|v| v.as_bool()), Some(true));
/// ```
#[doc(hidden)]
#[macro_export]
macro_rules! __akku_json {
    // ---- internal: array elements, separated by a semicolon from the vector --
    (@array $items:ident;) => {};
    (@array $items:ident; null , $($rest:tt)*) => {{
        $items.push($crate::json::Value::Null);
        $crate::__akku_json!(@array $items; $($rest)*);
    }};
    (@array $items:ident; null) => {{
        $items.push($crate::json::Value::Null);
    }};
    (@array $items:ident; [ $($inner:tt)* ] , $($rest:tt)*) => {{
        $items.push($crate::__akku_json!([ $($inner)* ]));
        $crate::__akku_json!(@array $items; $($rest)*);
    }};
    (@array $items:ident; [ $($inner:tt)* ]) => {{
        $items.push($crate::__akku_json!([ $($inner)* ]));
    }};
    (@array $items:ident; { $($inner:tt)* } , $($rest:tt)*) => {{
        $items.push($crate::__akku_json!({ $($inner)* }));
        $crate::__akku_json!(@array $items; $($rest)*);
    }};
    (@array $items:ident; { $($inner:tt)* }) => {{
        $items.push($crate::__akku_json!({ $($inner)* }));
    }};
    (@array $items:ident; $value:expr , $($rest:tt)*) => {{
        $items.push($crate::json::ToJson::to_json(&$value));
        $crate::__akku_json!(@array $items; $($rest)*);
    }};
    (@array $items:ident; $value:expr) => {{
        $items.push($crate::json::ToJson::to_json(&$value));
    }};

    // ---- internal: object entries -----------------------------------------
    (@object $object:ident;) => {};
    (@object $object:ident; $key:literal : null , $($rest:tt)*) => {{
        $object.insert($key, $crate::json::Value::Null);
        $crate::__akku_json!(@object $object; $($rest)*);
    }};
    (@object $object:ident; $key:literal : null) => {{
        $object.insert($key, $crate::json::Value::Null);
    }};
    (@object $object:ident; $key:literal : [ $($inner:tt)* ] , $($rest:tt)*) => {{
        $object.insert($key, $crate::__akku_json!([ $($inner)* ]));
        $crate::__akku_json!(@object $object; $($rest)*);
    }};
    (@object $object:ident; $key:literal : [ $($inner:tt)* ]) => {{
        $object.insert($key, $crate::__akku_json!([ $($inner)* ]));
    }};
    (@object $object:ident; $key:literal : { $($inner:tt)* } , $($rest:tt)*) => {{
        $object.insert($key, $crate::__akku_json!({ $($inner)* }));
        $crate::__akku_json!(@object $object; $($rest)*);
    }};
    (@object $object:ident; $key:literal : { $($inner:tt)* }) => {{
        $object.insert($key, $crate::__akku_json!({ $($inner)* }));
    }};
    (@object $object:ident; $key:literal : $value:expr , $($rest:tt)*) => {{
        $object.insert($key, $crate::json::ToJson::to_json(&$value));
        $crate::__akku_json!(@object $object; $($rest)*);
    }};
    (@object $object:ident; $key:literal : $value:expr) => {{
        $object.insert($key, $crate::json::ToJson::to_json(&$value));
    }};

    // ---- arrays -----------------------------------------------------------
    ([]) => { $crate::json::Value::Array(::std::vec::Vec::new()) };
    ([ $($tt:tt)+ ]) => {{
        let mut items: ::std::vec::Vec<$crate::json::Value> = ::std::vec::Vec::new();
        $crate::__akku_json!(@array items; $($tt)+);
        $crate::json::Value::Array(items)
    }};

    // ---- objects ----------------------------------------------------------
    ({}) => { $crate::json::Value::Object($crate::json::Object::new()) };
    ({ $($tt:tt)+ }) => {{
        let mut object = $crate::json::Object::new();
        $crate::__akku_json!(@object object; $($tt)+);
        $crate::json::Value::Object(object)
    }};

    // ---- scalars ----------------------------------------------------------
    (null) => { $crate::json::Value::Null };
    (true) => { $crate::json::Value::Bool(true) };
    (false) => { $crate::json::Value::Bool(false) };

    // ---- any other expression that implements ToJson ----------------------
    ($value:expr) => { $crate::json::ToJson::to_json(&$value) };
}

pub use crate::__akku_json as json;
