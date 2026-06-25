import json
import urllib.request


def load(path):
    with open(path) as f:
        return json.load(f)


def load_remote(url):
    return json.load(urllib.request.urlopen(url))
