"""Read settings by the shared wallboard contract. Transport credentials never enter this model."""
import copy
import json
import math
from pathlib import Path

CONTRACT = json.loads(Path(__file__).with_name('wallboard-contract.json').read_text())


def validate_settings(value, spec=CONTRACT, path='settings'):
    """A value the contract accepts, with its defaults filled in. Raises ValueError on anything else."""
    if value is None and spec.get('nullable'):
        return None
    kind = spec['type']
    valid = {'object': type(value) is dict, 'array': type(value) is list,
             'string': type(value) is str, 'boolean': type(value) is bool,
             'integer': type(value) in (int, float) and math.isfinite(value) and int(value) == value,
             'number': type(value) in (int, float) and math.isfinite(value)}[kind]
    if not valid:
        raise ValueError(f'{path}: expected {kind}')
    if kind == 'object':
        result = {key: validate_settings(value.get(key, copy.deepcopy(child['default'])), child, f'{path}.{key}')
                  for key, child in spec['properties'].items()}
        for flag, required in spec.get('required_when', {}).items():
            missing = [key for key in required if result[key] is None]
            if result[flag] and missing:
                raise ValueError(f'{path}: {flag} requires {", ".join(missing)}')
        return result
    if kind == 'array':
        if len(value) < spec.get('min_length', 0) or any(item not in spec['enum'] for item in value):
            raise ValueError(f'{path}: invalid selection')
        if spec.get('unique') and len({json.dumps(item) for item in value}) != len(value):
            raise ValueError(f'{path}: duplicate selection')
    elif 'enum' in spec and value not in spec['enum']:
        raise ValueError(f'{path}: unknown value')
    if kind in ('integer', 'number') and not spec.get('min', -math.inf) <= value <= spec.get('max', math.inf):
        raise ValueError(f'{path}: outside supported range')
    return int(value) if kind == 'integer' else copy.deepcopy(value)
