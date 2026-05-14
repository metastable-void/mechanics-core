use crate::internal::{executor::CustomModuleLoader, runtime::buffer_like};
use boa_engine::{
    Context, JsArgs, JsData, JsNativeError, JsObject, JsResult, JsSymbol, JsValue, Module,
    NativeFunction,
    class::{Class, ClassBuilder},
    js_string,
    module::SyntheticModuleInitializer,
    object::{
        FunctionObjectBuilder,
        builtins::{JsArray, JsFunction},
    },
    property::Attribute,
};
use boa_gc::{Finalize, Trace};
use std::{cell::RefCell, rc::Rc};
use url::{Url, form_urlencoded, quirks};

type UrlState = Rc<RefCell<Url>>;
type ParamPair = (String, String);

#[derive(Clone, Debug)]
enum ParamsBacking {
    Owned(Rc<RefCell<Vec<ParamPair>>>),
    Url(UrlState),
}

#[derive(Clone, Debug, Trace, Finalize, JsData)]
struct MechanicsUrl {
    // SAFETY: `url::Url` is Rust-owned parser state and does not embed Boa GC handles.
    #[unsafe_ignore_trace]
    inner: UrlState,
}

#[derive(Clone, Debug, Trace, Finalize, JsData)]
struct MechanicsUrlSearchParams {
    // SAFETY: Parameter storage is Rust-owned data and does not embed Boa GC handles. The
    // URL-backed variant reparses and writes the URL query on each operation so existing
    // `url.searchParams` objects stay bidirectionally bound after URL property mutations.
    #[unsafe_ignore_trace]
    backing: ParamsBacking,
}

fn js_type_error(message: impl Into<String>) -> boa_engine::JsError {
    let message: String = message.into();
    JsNativeError::typ().with_message(message).into()
}

fn required_js_string(args: &[JsValue], index: usize, name: &str) -> JsResult<String> {
    args.get_or_undefined(index)
        .as_string()
        .map(|s| s.to_std_string_lossy())
        .ok_or_else(|| js_type_error(format!("{name} must be a string")))
}

fn optional_js_string(args: &[JsValue], index: usize, name: &str) -> JsResult<Option<String>> {
    let value = args.get_or_undefined(index);
    if value.is_undefined() {
        return Ok(None);
    }
    value
        .as_string()
        .map(|s| Some(s.to_std_string_lossy()))
        .ok_or_else(|| js_type_error(format!("{name} must be a string when provided")))
}

fn js_string_value(value: impl AsRef<str>) -> JsValue {
    buffer_like::js_string_value(value.as_ref())
}

fn parse_url(input: &str, base: Option<&str>) -> Result<Url, url::ParseError> {
    if let Some(base) = base {
        Url::parse(base).and_then(|base_url| base_url.join(input))
    } else {
        Url::parse(input)
    }
}

fn this_url(this: &JsValue) -> JsResult<UrlState> {
    let Some(object) = this.as_object() else {
        return Err(js_type_error("invalid this for URL"));
    };
    object
        .downcast_ref::<MechanicsUrl>()
        .map(|url| url.inner.clone())
        .ok_or_else(|| js_type_error("invalid this for URL"))
}

fn this_params(this: &JsValue) -> JsResult<ParamsBacking> {
    let Some(object) = this.as_object() else {
        return Err(js_type_error("invalid this for URLSearchParams"));
    };
    object
        .downcast_ref::<MechanicsUrlSearchParams>()
        .map(|params| params.backing.clone())
        .ok_or_else(|| js_type_error("invalid this for URLSearchParams"))
}

fn read_query_pairs(url: &Url) -> Vec<ParamPair> {
    url.query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn encode_pairs(pairs: &[ParamPair]) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    serializer.extend_pairs(
        pairs
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    );
    serializer.finish()
}

fn write_query_pairs(url: &mut Url, pairs: &[ParamPair]) {
    if pairs.is_empty() {
        url.set_query(None);
    } else {
        url.set_query(Some(&encode_pairs(pairs)));
    }
}

fn with_pairs<R>(backing: &ParamsBacking, f: impl FnOnce(&mut Vec<ParamPair>) -> R) -> R {
    match backing {
        ParamsBacking::Owned(pairs) => f(&mut pairs.borrow_mut()),
        ParamsBacking::Url(url) => {
            let mut url = url.borrow_mut();
            let mut pairs = read_query_pairs(&url);
            let result = f(&mut pairs);
            write_query_pairs(&mut url, &pairs);
            result
        }
    }
}

fn snapshot_pairs(backing: &ParamsBacking) -> Vec<ParamPair> {
    match backing {
        ParamsBacking::Owned(pairs) => pairs.borrow().clone(),
        ParamsBacking::Url(url) => read_query_pairs(&url.borrow()),
    }
}

fn parse_params_string(input: &str) -> Vec<ParamPair> {
    let input = input.strip_prefix('?').unwrap_or(input);
    form_urlencoded::parse(input.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

fn is_array_like(object: &JsObject, context: &mut Context) -> JsResult<bool> {
    Ok(!object.get(js_string!("length"), context)?.is_undefined())
}

fn collect_pair(value: &JsValue, context: &mut Context) -> JsResult<ParamPair> {
    let object = value
        .as_object()
        .ok_or_else(|| js_type_error("URLSearchParams iterable items must be objects"))?;
    let name = object
        .get(0, context)?
        .as_string()
        .map(|s| s.to_std_string_lossy())
        .ok_or_else(|| js_type_error("URLSearchParams pair name must be a string"))?;
    let value = object
        .get(1, context)?
        .as_string()
        .map(|s| s.to_std_string_lossy())
        .ok_or_else(|| js_type_error("URLSearchParams pair value must be a string"))?;
    Ok((name, value))
}

fn collect_object_pairs(object: &JsObject, context: &mut Context) -> JsResult<Vec<ParamPair>> {
    let mut pairs = Vec::new();
    for key in object.own_property_keys(context)? {
        let name = key.to_string();
        let value = object
            .get(key, context)?
            .as_string()
            .map(|s| s.to_std_string_lossy())
            .ok_or_else(|| js_type_error("URLSearchParams object values must be strings"))?;
        pairs.push((name, value));
    }
    Ok(pairs)
}

fn collect_array_pairs(object: &JsObject, context: &mut Context) -> JsResult<Vec<ParamPair>> {
    let len = object
        .get(js_string!("length"), context)?
        .to_length(context)?;
    let mut pairs = Vec::new();
    for index in 0..len {
        pairs.push(collect_pair(&object.get(index, context)?, context)?);
    }
    Ok(pairs)
}

fn make_accessor(
    context: &mut Context,
    name: &'static str,
    function: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
) -> JsFunction {
    FunctionObjectBuilder::new(context.realm(), NativeFunction::from_fn_ptr(function))
        .name(name)
        .length(0)
        .constructor(false)
        .build()
}

fn apply_url_setter(
    this: &JsValue,
    args: &[JsValue],
    name: &str,
    setter: impl FnOnce(&mut Url, &str) -> Result<(), ()>,
) -> JsResult<JsValue> {
    let value = required_js_string(args, 0, name)?;
    let url = this_url(this)?;
    setter(&mut url.borrow_mut(), &value)
        .map_err(|()| js_type_error(format!("invalid URL {name}")))?;
    Ok(JsValue::undefined())
}

fn url_href_get(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::href(&url.borrow())))
}

fn url_href_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let value = required_js_string(args, 0, "href")?;
    let url = this_url(this)?;
    quirks::set_href(&mut url.borrow_mut(), &value)
        .map_err(|_| js_type_error("invalid URL href"))?;
    Ok(JsValue::undefined())
}

fn url_origin_get(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::origin(&url.borrow())))
}

fn url_protocol_get(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::protocol(&url.borrow())))
}

fn url_protocol_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    apply_url_setter(this, args, "protocol", quirks::set_protocol)
}

fn url_username_get(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::username(&url.borrow())))
}

fn url_username_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    apply_url_setter(this, args, "username", quirks::set_username)
}

fn url_password_get(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::password(&url.borrow())))
}

fn url_password_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    apply_url_setter(this, args, "password", quirks::set_password)
}

fn url_host_get(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::host(&url.borrow())))
}

fn url_host_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    apply_url_setter(this, args, "host", quirks::set_host)
}

fn url_hostname_get(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::hostname(&url.borrow())))
}

fn url_hostname_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    apply_url_setter(this, args, "hostname", quirks::set_hostname)
}

fn url_port_get(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::port(&url.borrow())))
}

fn url_port_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    apply_url_setter(this, args, "port", quirks::set_port)
}

fn url_pathname_get(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::pathname(&url.borrow())))
}

fn url_pathname_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let value = required_js_string(args, 0, "pathname")?;
    let url = this_url(this)?;
    quirks::set_pathname(&mut url.borrow_mut(), &value);
    Ok(JsValue::undefined())
}

fn url_search_get(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::search(&url.borrow())))
}

fn url_search_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let value = required_js_string(args, 0, "search")?;
    let url = this_url(this)?;
    quirks::set_search(&mut url.borrow_mut(), &value);
    Ok(JsValue::undefined())
}

fn url_search_params_get(
    this: &JsValue,
    _args: &[JsValue],
    context: &mut Context,
) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(MechanicsUrlSearchParams {
        backing: ParamsBacking::Url(url),
    }
    .into_js_object(context)?
    .into())
}

fn url_hash_get(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let url = this_url(this)?;
    Ok(js_string_value(quirks::hash(&url.borrow())))
}

fn url_hash_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let value = required_js_string(args, 0, "hash")?;
    let url = this_url(this)?;
    quirks::set_hash(&mut url.borrow_mut(), &value);
    Ok(JsValue::undefined())
}

fn url_to_string(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    url_href_get(this, &[], _context)
}

fn url_can_parse(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let input = required_js_string(args, 0, "input")?;
    let base = optional_js_string(args, 1, "base")?;
    Ok(JsValue::new(parse_url(&input, base.as_deref()).is_ok()))
}

fn url_parse(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let input = required_js_string(args, 0, "input")?;
    let base = optional_js_string(args, 1, "base")?;
    match parse_url(&input, base.as_deref()) {
        Ok(url) => Ok(MechanicsUrl {
            inner: Rc::new(RefCell::new(url)),
        }
        .into_js_object(context)?
        .into()),
        Err(_) => Ok(JsValue::null()),
    }
}

fn array_values_iterator(array: JsArray, context: &mut Context) -> JsResult<JsValue> {
    let array_value = JsValue::from(array.clone());
    let values = array
        .get(js_string!("values"), context)?
        .as_callable()
        .ok_or_else(|| js_type_error("Array.prototype.values is not callable"))?;
    values.call(&array_value, &[], context)
}

fn pair_array(name: String, value: String, context: &mut Context) -> JsValue {
    JsArray::from_iter([js_string_value(name), js_string_value(value)], context).into()
}

fn params_entries(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let backing = this_params(this)?;
    let entries: Vec<_> = snapshot_pairs(&backing)
        .into_iter()
        .map(|(name, value)| pair_array(name, value, context))
        .collect();
    array_values_iterator(JsArray::from_iter(entries, context), context)
}

fn params_keys(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let backing = this_params(this)?;
    let keys = snapshot_pairs(&backing)
        .into_iter()
        .map(|(name, _)| js_string_value(name));
    array_values_iterator(JsArray::from_iter(keys, context), context)
}

fn params_values(this: &JsValue, _args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let backing = this_params(this)?;
    let values = snapshot_pairs(&backing)
        .into_iter()
        .map(|(_, value)| js_string_value(value));
    array_values_iterator(JsArray::from_iter(values, context), context)
}

fn params_append(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let name = required_js_string(args, 0, "name")?;
    let value = required_js_string(args, 1, "value")?;
    let backing = this_params(this)?;
    with_pairs(&backing, |pairs| pairs.push((name, value)));
    Ok(JsValue::undefined())
}

fn params_delete(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let name = required_js_string(args, 0, "name")?;
    let value = optional_js_string(args, 1, "value")?;
    let backing = this_params(this)?;
    with_pairs(&backing, |pairs| {
        pairs.retain(|(pair_name, pair_value)| {
            pair_name != &name || value.as_ref().is_some_and(|value| pair_value != value)
        });
    });
    Ok(JsValue::undefined())
}

fn params_get(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let name = required_js_string(args, 0, "name")?;
    let backing = this_params(this)?;
    Ok(snapshot_pairs(&backing)
        .into_iter()
        .find(|(pair_name, _)| pair_name == &name)
        .map_or_else(JsValue::null, |(_, value)| js_string_value(value)))
}

fn params_get_all(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let name = required_js_string(args, 0, "name")?;
    let backing = this_params(this)?;
    let values = snapshot_pairs(&backing)
        .into_iter()
        .filter_map(|(pair_name, value)| (pair_name == name).then(|| js_string_value(value)));
    Ok(JsArray::from_iter(values, context).into())
}

fn params_has(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let name = required_js_string(args, 0, "name")?;
    let value = optional_js_string(args, 1, "value")?;
    let backing = this_params(this)?;
    Ok(JsValue::new(snapshot_pairs(&backing).into_iter().any(
        |(pair_name, pair_value)| {
            pair_name == name && value.as_ref().is_none_or(|value| pair_value == *value)
        },
    )))
}

fn params_set(this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let name = required_js_string(args, 0, "name")?;
    let value = required_js_string(args, 1, "value")?;
    let backing = this_params(this)?;
    with_pairs(&backing, |pairs| {
        let mut seen = false;
        pairs.retain_mut(|(pair_name, pair_value)| {
            if pair_name == &name {
                if seen {
                    false
                } else {
                    *pair_value = value.clone();
                    seen = true;
                    true
                }
            } else {
                true
            }
        });
        if !seen {
            pairs.push((name, value));
        }
    });
    Ok(JsValue::undefined())
}

fn params_sort(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let backing = this_params(this)?;
    with_pairs(&backing, |pairs| {
        pairs.sort_by(|left, right| left.0.cmp(&right.0))
    });
    Ok(JsValue::undefined())
}

fn params_to_string(
    this: &JsValue,
    _args: &[JsValue],
    _context: &mut Context,
) -> JsResult<JsValue> {
    let backing = this_params(this)?;
    Ok(js_string_value(encode_pairs(&snapshot_pairs(&backing))))
}

fn params_for_each(this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let callback = args
        .get_or_undefined(0)
        .as_callable()
        .ok_or_else(|| js_type_error("callback must be callable"))?;
    let this_arg = args.get_or_undefined(1).clone();
    let backing = this_params(this)?;
    for (name, value) in snapshot_pairs(&backing) {
        callback.call(
            &this_arg,
            &[js_string_value(value), js_string_value(name), this.clone()],
            context,
        )?;
    }
    Ok(JsValue::undefined())
}

fn params_size_get(this: &JsValue, _args: &[JsValue], _context: &mut Context) -> JsResult<JsValue> {
    let backing = this_params(this)?;
    let len = snapshot_pairs(&backing).len();
    let size = i32::try_from(len).map_err(|_| js_type_error("URLSearchParams size overflow"))?;
    Ok(JsValue::new(size))
}

impl MechanicsUrl {
    fn into_js_object(self, context: &mut Context) -> JsResult<JsObject> {
        Self::from_data(self, context)
    }
}

impl Class for MechanicsUrl {
    const NAME: &'static str = "URL";
    const LENGTH: usize = 1;

    fn data_constructor(
        _new_target: &JsValue,
        args: &[JsValue],
        _context: &mut Context,
    ) -> JsResult<Self> {
        let input = required_js_string(args, 0, "input")?;
        let base = optional_js_string(args, 1, "base")?;
        let parsed =
            parse_url(&input, base.as_deref()).map_err(|_| js_type_error("invalid URL"))?;
        Ok(Self {
            inner: Rc::new(RefCell::new(parsed)),
        })
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        let attr = Attribute::CONFIGURABLE | Attribute::ENUMERABLE;

        let href_get = make_accessor(class.context(), "get href", url_href_get);
        let href_set = make_accessor(class.context(), "set href", url_href_set);
        class.accessor(js_string!("href"), Some(href_get), Some(href_set), attr);

        let origin_get = make_accessor(class.context(), "get origin", url_origin_get);
        class.accessor(js_string!("origin"), Some(origin_get), None, attr);

        let protocol_get = make_accessor(class.context(), "get protocol", url_protocol_get);
        let protocol_set = make_accessor(class.context(), "set protocol", url_protocol_set);
        class.accessor(
            js_string!("protocol"),
            Some(protocol_get),
            Some(protocol_set),
            attr,
        );

        let username_get = make_accessor(class.context(), "get username", url_username_get);
        let username_set = make_accessor(class.context(), "set username", url_username_set);
        class.accessor(
            js_string!("username"),
            Some(username_get),
            Some(username_set),
            attr,
        );

        let password_get = make_accessor(class.context(), "get password", url_password_get);
        let password_set = make_accessor(class.context(), "set password", url_password_set);
        class.accessor(
            js_string!("password"),
            Some(password_get),
            Some(password_set),
            attr,
        );

        let host_get = make_accessor(class.context(), "get host", url_host_get);
        let host_set = make_accessor(class.context(), "set host", url_host_set);
        class.accessor(js_string!("host"), Some(host_get), Some(host_set), attr);

        let hostname_get = make_accessor(class.context(), "get hostname", url_hostname_get);
        let hostname_set = make_accessor(class.context(), "set hostname", url_hostname_set);
        class.accessor(
            js_string!("hostname"),
            Some(hostname_get),
            Some(hostname_set),
            attr,
        );

        let port_get = make_accessor(class.context(), "get port", url_port_get);
        let port_set = make_accessor(class.context(), "set port", url_port_set);
        class.accessor(js_string!("port"), Some(port_get), Some(port_set), attr);

        let pathname_get = make_accessor(class.context(), "get pathname", url_pathname_get);
        let pathname_set = make_accessor(class.context(), "set pathname", url_pathname_set);
        class.accessor(
            js_string!("pathname"),
            Some(pathname_get),
            Some(pathname_set),
            attr,
        );

        let search_get = make_accessor(class.context(), "get search", url_search_get);
        let search_set = make_accessor(class.context(), "set search", url_search_set);
        class.accessor(
            js_string!("search"),
            Some(search_get),
            Some(search_set),
            attr,
        );

        let search_params_get =
            make_accessor(class.context(), "get searchParams", url_search_params_get);
        class.accessor(
            js_string!("searchParams"),
            Some(search_params_get),
            None,
            attr,
        );

        let hash_get = make_accessor(class.context(), "get hash", url_hash_get);
        let hash_set = make_accessor(class.context(), "set hash", url_hash_set);
        class.accessor(js_string!("hash"), Some(hash_get), Some(hash_set), attr);

        class.method(
            js_string!("toString"),
            0,
            NativeFunction::from_fn_ptr(url_to_string),
        );
        class.method(
            js_string!("toJSON"),
            0,
            NativeFunction::from_fn_ptr(url_to_string),
        );
        class.static_method(
            js_string!("canParse"),
            1,
            NativeFunction::from_fn_ptr(url_can_parse),
        );
        class.static_method(
            js_string!("parse"),
            1,
            NativeFunction::from_fn_ptr(url_parse),
        );
        Ok(())
    }
}

impl MechanicsUrlSearchParams {
    fn into_js_object(self, context: &mut Context) -> JsResult<JsObject> {
        Self::from_data(self, context)
    }
}

impl Class for MechanicsUrlSearchParams {
    const NAME: &'static str = "URLSearchParams";
    const LENGTH: usize = 0;

    fn data_constructor(
        _new_target: &JsValue,
        args: &[JsValue],
        context: &mut Context,
    ) -> JsResult<Self> {
        let init = args.get_or_undefined(0);
        let pairs = if init.is_undefined() || init.is_null() {
            Vec::new()
        } else if let Some(text) = init.as_string() {
            parse_params_string(&text.to_std_string_lossy())
        } else if let Some(object) = init.as_object() {
            if is_array_like(&object, context)? {
                collect_array_pairs(&object, context)?
            } else {
                collect_object_pairs(&object, context)?
            }
        } else {
            return Err(js_type_error(
                "URLSearchParams init must be a string or object",
            ));
        };
        Ok(Self {
            backing: ParamsBacking::Owned(Rc::new(RefCell::new(pairs))),
        })
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            js_string!("append"),
            2,
            NativeFunction::from_fn_ptr(params_append),
        );
        class.method(
            js_string!("delete"),
            1,
            NativeFunction::from_fn_ptr(params_delete),
        );
        class.method(
            js_string!("get"),
            1,
            NativeFunction::from_fn_ptr(params_get),
        );
        class.method(
            js_string!("getAll"),
            1,
            NativeFunction::from_fn_ptr(params_get_all),
        );
        class.method(
            js_string!("has"),
            1,
            NativeFunction::from_fn_ptr(params_has),
        );
        class.method(
            js_string!("set"),
            2,
            NativeFunction::from_fn_ptr(params_set),
        );
        class.method(
            js_string!("sort"),
            0,
            NativeFunction::from_fn_ptr(params_sort),
        );
        class.method(
            js_string!("toString"),
            0,
            NativeFunction::from_fn_ptr(params_to_string),
        );
        class.method(
            js_string!("entries"),
            0,
            NativeFunction::from_fn_ptr(params_entries),
        );
        class.method(
            js_string!("keys"),
            0,
            NativeFunction::from_fn_ptr(params_keys),
        );
        class.method(
            js_string!("values"),
            0,
            NativeFunction::from_fn_ptr(params_values),
        );
        class.method(
            js_string!("forEach"),
            1,
            NativeFunction::from_fn_ptr(params_for_each),
        );
        class.method(
            JsSymbol::iterator(),
            0,
            NativeFunction::from_fn_ptr(params_entries),
        );
        let size_get = make_accessor(class.context(), "get size", params_size_get);
        class.accessor(
            js_string!("size"),
            Some(size_get),
            None,
            Attribute::CONFIGURABLE | Attribute::ENUMERABLE,
        );
        Ok(())
    }
}

fn register_class<C: Class>(context: &mut Context) -> JsResult<JsObject> {
    if let Some(class) = context.get_global_class::<C>() {
        return Ok(class.constructor());
    }

    let mut class_builder = ClassBuilder::new::<C>(context);
    C::init(&mut class_builder)?;
    let class = class_builder.build();
    let constructor = class.constructor();
    context.realm().register_class::<C>(class);
    Ok(constructor)
}

pub(super) fn register(loader: &Rc<CustomModuleLoader>, context: &mut Context) {
    let Ok(url_constructor) = register_class::<MechanicsUrl>(context) else {
        return;
    };
    let Ok(params_constructor) = register_class::<MechanicsUrlSearchParams>(context) else {
        return;
    };

    let url_module = Module::synthetic(
        &[js_string!("default"), js_string!("URLSearchParams")],
        SyntheticModuleInitializer::from_copy_closure_with_captures(
            |module, constructors, _ctx| {
                module.set_export(&js_string!("default"), constructors.0.clone().into())?;
                module.set_export(
                    &js_string!("URLSearchParams"),
                    constructors.1.clone().into(),
                )
            },
            (url_constructor, params_constructor),
        ),
        None,
        None,
        context,
    );
    loader.define_module(js_string!("mechanics:url"), url_module);
}
