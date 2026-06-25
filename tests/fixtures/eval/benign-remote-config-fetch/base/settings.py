import json


def load(path):
    with open(path) as f:
        return json.load(f)
