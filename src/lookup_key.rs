use std::fmt;

use pyo3::exceptions::{PyAttributeError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyString};

use crate::build_tools::{py_error, SchemaDict};
use crate::input::{JsonInput, JsonObject};

/// Used got getting items from python dicts, python objects, or JSON objects, in different ways
#[derive(Debug, Clone)]
pub enum LookupKey {
    /// simply look up a key in a dict, equivalent to `d.get(key)`
    /// we save both the string and pystring to save creating the pystring for python
    Simple(String, Py<PyString>),
    /// look up a key by either string, equivalent to `d.get(choice1, d.get(choice2))`
    /// these are interpreted as (json_key1, json_key2, py_key1, py_key2)
    Choice(String, String, Py<PyString>, Py<PyString>),
    /// look up keys buy one or more "paths" a path might be `['foo', 'bar']` to get `d.?foo.?bar`
    /// ints are also supported to index arrays/lists/tuples and dicts with int keys
    /// we reuse Location as the enum is the same, and the meaning is the same
    PathChoices(Vec<Path>),
}

impl fmt::Display for LookupKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Simple(key, _) => write!(f, "{}", key),
            Self::Choice(key1, key2, _, _) => write!(f, "{} | {}", key1, key2),
            Self::PathChoices(paths) => write!(
                f,
                "{}",
                paths.iter().map(path_to_string).collect::<Vec<_>>().join(" | ")
            ),
        }
    }
}

macro_rules! py_string {
    ($py:ident, $str:expr) => {
        PyString::intern($py, $str).into()
    };
}

impl LookupKey {
    pub fn from_py(
        py: Python,
        field: &PyDict,
        alt_alias: Option<&str>,
        single_name: &str,
        plural_name: &str,
    ) -> PyResult<Option<Self>> {
        match field.get_as::<String>(single_name)? {
            Some(alias) => {
                if field.contains(plural_name)? {
                    py_error!("'{}' and '{}' cannot be used together", single_name, plural_name)
                } else {
                    let alias_py = py_string!(py, &alias);
                    match alt_alias {
                        Some(alt_alias) => Ok(Some(LookupKey::Choice(
                            alias,
                            alt_alias.to_string(),
                            alias_py,
                            py_string!(py, alt_alias),
                        ))),
                        None => Ok(Some(LookupKey::Simple(alias, alias_py))),
                    }
                }
            }
            None => match field.get_as::<&PyList>(plural_name)? {
                Some(aliases) => {
                    let mut locs = aliases
                        .iter()
                        .map(|obj| Self::path_choice(py, obj))
                        .collect::<PyResult<Vec<Path>>>()?;

                    if locs.is_empty() {
                        py_error!("{} must have at least one element", plural_name)
                    } else {
                        if let Some(alt_alias) = alt_alias {
                            locs.push(vec![PathItem::S(alt_alias.to_string(), py_string!(py, alt_alias))])
                        }
                        Ok(Some(LookupKey::PathChoices(locs)))
                    }
                }
                None => Ok(None),
            },
        }
    }

    pub fn from_string(py: Python, key: &str) -> Self {
        LookupKey::Simple(key.to_string(), py_string!(py, key))
    }

    fn path_choice(py: Python, obj: &PyAny) -> PyResult<Path> {
        let path = obj
            .extract::<&PyList>()?
            .iter()
            .enumerate()
            .map(|(index, obj)| PathItem::from_py(py, index, obj))
            .collect::<PyResult<Path>>()?;

        if path.is_empty() {
            py_error!("Each alias path must have at least one element")
        } else {
            Ok(path)
        }
    }

    pub fn py_get_item<'data, 's>(&'s self, dict: &'data PyDict) -> PyResult<Option<(&'s str, &'data PyAny)>> {
        match self {
            LookupKey::Simple(key, py_key) => match dict.get_item(py_key) {
                Some(value) => Ok(Some((key, value))),
                None => Ok(None),
            },
            LookupKey::Choice(key1, key2, py_key1, py_key2) => match dict.get_item(py_key1) {
                Some(value) => Ok(Some((key1, value))),
                None => match dict.get_item(py_key2) {
                    Some(value) => Ok(Some((key2, value))),
                    None => Ok(None),
                },
            },
            LookupKey::PathChoices(path_choices) => {
                for path in path_choices {
                    // iterate over the path and plug each value into the py_any from the last step, starting with dict
                    // this could just be a loop but should be somewhat faster with a functional design
                    if let Some(v) = path.iter().try_fold(dict as &PyAny, |d, loc| loc.py_get_item(d)) {
                        // Successfully found an item, return it
                        let key = path.first().unwrap().get_key();
                        return Ok(Some((key, v)));
                    }
                }
                // got to the end of path_choices, without a match, return None
                Ok(None)
            }
        }
    }

    pub fn py_get_attr<'data, 's>(&'s self, obj: &'data PyAny) -> PyResult<Option<(&'s str, &'data PyAny)>> {
        match self {
            LookupKey::Simple(key, py_key) => match py_get_attrs(obj, &py_key)? {
                Some(value) => Ok(Some((key, value))),
                None => Ok(None),
            },
            LookupKey::Choice(key1, key2, py_key1, py_key2) => match py_get_attrs(obj, &py_key1)? {
                Some(value) => Ok(Some((key1, value))),
                None => match py_get_attrs(obj, &py_key2)? {
                    Some(value) => Ok(Some((key2, value))),
                    None => Ok(None),
                },
            },
            LookupKey::PathChoices(path_choices) => {
                'outer: for path in path_choices {
                    // similar to above, but using `py_get_attrs`, we can't use try_fold because of the extra Err
                    // so we have to loop manually
                    let mut v = obj;
                    for loc in path {
                        v = match loc.py_get_attrs(v) {
                            Ok(Some(v)) => v,
                            Ok(None) => {
                                continue 'outer;
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    // Successfully found an item, return it
                    let key = path.first().unwrap().get_key();
                    return Ok(Some((key, v)));
                }
                // got to the end of path_choices, without a match, return None
                Ok(None)
            }
        }
    }

    pub fn json_get<'data, 's>(&'s self, dict: &'data JsonObject) -> PyResult<Option<(&'s str, &'data JsonInput)>> {
        match self {
            LookupKey::Simple(key, _) => match dict.get(key) {
                Some(value) => Ok(Some((key, value))),
                None => Ok(None),
            },
            LookupKey::Choice(key1, key2, _, _) => match dict.get(key1) {
                Some(value) => Ok(Some((key1, value))),
                None => match dict.get(key2) {
                    Some(value) => Ok(Some((key2, value))),
                    None => Ok(None),
                },
            },
            LookupKey::PathChoices(path_choices) => {
                for path in path_choices {
                    let mut path_iter = path.iter();

                    // first step is different from the rest as we already know dict is JsonObject
                    // because of above checks, we know that path must have at least one element, hence unwrap
                    let v: &JsonInput = match path_iter.next().unwrap().json_obj_get(dict) {
                        Some(v) => v,
                        None => continue,
                    };

                    // similar to above
                    // iterate over the path and plug each value into the JsonInput from the last step, starting with v
                    // from the first step, this could just be a loop but should be somewhat faster with a functional design
                    if let Some(v) = path_iter.try_fold(v, |d, loc| loc.json_get(d)) {
                        // Successfully found an item, return it
                        let key = path.first().unwrap().get_key();
                        return Ok(Some((key, v)));
                    }
                }
                // got to the end of path_choices, without a match, return None
                Ok(None)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum PathItem {
    /// string type key, used to get or identify items from a dict or anything that implements `__getitem__`
    /// as above we store both the string and pystring to save creating the pystring for python
    S(String, Py<PyString>),
    /// integer key, used to get items from a list, tuple OR a dict with int keys `Dict[int, ...]` (python only)
    I(usize),
}

impl fmt::Display for PathItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::S(key, _) => write!(f, "{}", key),
            Self::I(key) => write!(f, "{}", key),
        }
    }
}

impl ToPyObject for PathItem {
    fn to_object(&self, py: Python<'_>) -> PyObject {
        match self {
            Self::S(_, val) => val.to_object(py),
            Self::I(val) => val.to_object(py),
        }
    }
}

type Path = Vec<PathItem>;

fn path_to_string(path: &Path) -> String {
    path.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".")
}

impl PathItem {
    pub fn from_py(py: Python, index: usize, obj: &PyAny) -> PyResult<Self> {
        if let Ok(str_key) = obj.extract::<String>() {
            let py_str_key = py_string!(py, &str_key);
            Ok(Self::S(str_key, py_str_key))
        } else if let Ok(int_key) = obj.extract::<usize>() {
            if index == 0 {
                py_error!(PyTypeError; "The first item in an alias path must be a string")
            } else {
                Ok(Self::I(int_key))
            }
        } else {
            py_error!(PyTypeError; "Alias path items must be with a string or int")
        }
    }

    pub fn py_get_item<'a>(&self, py_any: &'a PyAny) -> Option<&'a PyAny> {
        // we definitely don't want to index strings, so explicitly omit this case
        if py_any.cast_as::<PyString>().is_ok() {
            None
        } else {
            // otherwise, blindly try getitem on v since no better logic is realistic
            py_any.get_item(self).ok()
        }
    }

    pub fn get_key(&self) -> &str {
        match self {
            Self::S(key, _) => key.as_str(),
            Self::I(_) => unreachable!(),
        }
    }

    pub fn py_get_attrs<'a>(&self, obj: &'a PyAny) -> PyResult<Option<&'a PyAny>> {
        match self {
            Self::S(_, py_key) => {
                // if obj is a dict, we want to use get_item, not getattr
                if obj.cast_as::<PyDict>().is_ok() {
                    Ok(self.py_get_item(obj))
                } else {
                    py_get_attrs(obj, py_key)
                }
            }
            // int, we fall back to py_get_item - e.g. we want to use get_item for a list, tuple, dict, etc.
            Self::I(_) => Ok(self.py_get_item(obj)),
        }
    }

    pub fn json_get<'a>(&self, any_json: &'a JsonInput) -> Option<&'a JsonInput> {
        match any_json {
            JsonInput::Object(v_obj) => self.json_obj_get(v_obj),
            JsonInput::Array(v_array) => match self {
                Self::I(index) => v_array.get(*index),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn json_obj_get<'a>(&self, json_obj: &'a JsonObject) -> Option<&'a JsonInput> {
        match self {
            Self::S(key, _) => json_obj.get(key),
            _ => None,
        }
    }
}

/// wrapper around `getattr` that returns `Ok(None)` for attribute errors, but returns other errors
/// We dont check `try_from_attributes` because that check was performed on the top level object before we got here
fn py_get_attrs<N>(obj: &PyAny, attr_name: N) -> PyResult<Option<&PyAny>>
where
    N: ToPyObject,
{
    match obj.getattr(attr_name) {
        Ok(attr) => Ok(Some(attr)),
        Err(err) => {
            if err.get_type(obj.py()).is_subclass_of::<PyAttributeError>()? {
                Ok(None)
            } else {
                Err(err)
            }
        }
    }
}
