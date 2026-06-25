import importlib

PLUGINS = {}


def register(name, fn):
    PLUGINS[name] = fn


def load(name):
    return importlib.import_module(f"app.plugins.{name}")
