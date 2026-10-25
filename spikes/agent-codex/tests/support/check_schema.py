#!/usr/bin/env python3
"""Structural fixture check against locally generated schemas (stdlib only).

Checks types, required fields, enums, refs and alternatives; this is deliberately
not a full JSON Schema implementation. Generate schema/ with the installed CLI.
"""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCHEMA = ROOT / 'schema'


def validate(value, schema, root, path=''):
    if schema is True:
        return
    if schema is False:
        raise ValueError(path + ': forbidden')
    if '$ref' in schema:
        target = root
        for key in schema['$ref'].split('/')[1:]:
            target = target[key]
        return validate(value, target, root, path)
    if 'enum' in schema and value not in schema['enum']:
        raise ValueError(path + ': invalid enum ' + repr(value))
    for kind in ('anyOf', 'oneOf'):
        if kind in schema:
            errors = []
            choices = schema[kind]
            if isinstance(value, dict):
                for discriminator in ('method', 'type'):
                    matching = [choice for choice in choices if value.get(discriminator) in choice.get('properties', {}).get(discriminator, {}).get('enum', [])]
                    if matching:
                        choices = matching
                        break
            for choice in choices:
                try:
                    validate(value, choice, root, path)
                    break
                except ValueError as error:
                    errors.append(str(error))
            else:
                raise ValueError(path + ': no matching alternative: ' + repr(errors)[:500])
    for clause in schema.get('allOf', []):
        validate(value, clause, root, path)
    types = schema.get('type')
    types = [types] if isinstance(types, str) else types
    actual = {type(None): 'null', bool: 'boolean', int: 'integer', float: 'number',
              str: 'string', list: 'array', dict: 'object'}[type(value)]
    if types and actual not in types and not (actual == 'integer' and 'number' in types):
        raise ValueError(path + ': expected ' + repr(types) + ', got ' + actual)
    if isinstance(value, dict):
        for key in schema.get('required', []):
            if key not in value:
                raise ValueError(path + ': missing ' + key)
        for key, item in value.items():
            if key in schema.get('properties', {}):
                validate(item, schema['properties'][key], root, path + '/' + key)
            elif isinstance(schema.get('additionalProperties'), dict):
                validate(item, schema['additionalProperties'], root, path + '/' + key)
            elif schema.get('additionalProperties') is False:
                raise ValueError(path + ': unknown ' + key)
    if isinstance(value, list):
        for index, item in enumerate(value):
            validate(item, schema.get('items', {}), root, path + '/' + str(index))


def load(name):
    return json.loads((SCHEMA / (name + '.json')).read_text())


client, server, request = [load(n) for n in ('ClientRequest', 'ServerNotification', 'ServerRequest')]
results = {'initialize': 'v1/InitializeResponse', 'thread/start': 'v2/ThreadStartResponse',
           'thread/resume': 'v2/ThreadResumeResponse', 'turn/start': 'v2/TurnStartResponse'}
errors, count = [], 0
for fixture in sorted((ROOT.parents[1] / 'fixtures/agents/codex').glob('*.jsonl')):
    rpc = {}
    for index, line in enumerate(fixture.read_text().splitlines(), 1):
        line = json.loads(line)
        if line['dir'] not in ('stdin', 'stdout'):
            continue
        value = json.loads(line['line'])
        method, spec = value.get('method'), None
        if method and 'id' in value and line['dir'] == 'stdin':
            spec = client
            rpc[str(value['id'])] = method
        elif method and 'id' in value:
            spec = request
        elif method and line['dir'] == 'stdout' and method != 'future/notification':
            spec = server
        elif line['dir'] == 'stdout' and 'result' in value:
            name = results.get(rpc.get(str(value.get('id'))))
            if name:
                spec, value = load(name), value['result']
        if spec:
            try:
                validate(value, spec, spec)
                count += 1
            except ValueError as error:
                errors.append(f'{fixture.name}:{index}: {error}')
print(f'{count} records passed structural schema checks; {len(errors)} errors')
for error in errors:
    print(error)
raise SystemExit(bool(errors))
