import pickle

import pytest

from pydantic_core import SchemaError, SchemaValidator


def test_build_error_type():
    with pytest.raises(SchemaError, match='Unknown schema type: "foobar"'):
        SchemaValidator({'type': 'foobar', 'title': 'TestModel'})


def test_build_error_internal():
    msg = (
        'Error building "str" validator:\n'
        '  TypeError: \'str\' object cannot be interpreted as an integer'  # noqa Q003
    )
    with pytest.raises(SchemaError, match=msg):
        SchemaValidator({'type': 'str', 'min_length': 'xxx', 'title': 'TestModel'})


def test_build_error_deep():
    with pytest.raises(SchemaError) as exc_info:
        SchemaValidator(
            {
                'title': 'MyTestModel',
                'type': 'typed-dict',
                'fields': {'age': {'schema': {'type': 'int', 'ge': 'not-int'}}},
            }
        )
    assert str(exc_info.value) == (
        'Error building "typed-dict" validator:\n'
        '  SchemaError: Field "age":\n'
        '  SchemaError: Error building "int" validator:\n'
        "  TypeError: 'str' object cannot be interpreted as an integer"
    )


def test_schema_as_string():
    v = SchemaValidator('bool')
    assert v.validate_python('tRuE') is True


def test_schema_wrong_type():
    with pytest.raises(SchemaError) as exc_info:
        SchemaValidator(1)
    assert exc_info.value.args[0] == (
        "Schema build error:\n  TypeError: 'int' object cannot be converted to 'PyString'"
    )


@pytest.mark.parametrize('pickle_protocol', range(1, pickle.HIGHEST_PROTOCOL + 1))
def test_pickle(pickle_protocol: int) -> None:
    v1 = SchemaValidator({'type': 'bool'})
    assert v1.validate_python('tRuE') is True
    p = pickle.dumps(v1, protocol=pickle_protocol)
    v2 = pickle.loads(p)
    assert v2.validate_python('tRuE') is True
    assert repr(v1) == repr(v2)


def test_schema_recursive_error():
    schema = {'type': 'union', 'choices': []}
    schema['choices'].append({'type': 'nullable', 'schema': schema})
    with pytest.raises(SchemaError, match='Recursion error - cyclic reference detected'):
        SchemaValidator(schema)


def test_not_schema_recursive_error():
    schema = {
        'type': 'typed-dict',
        'fields': {f'f_{i}': {'schema': {'type': 'nullable', 'schema': 'int'}} for i in range(101)},
    }
    v = SchemaValidator(schema)
    assert repr(v).count('TypedDictField') == 101
