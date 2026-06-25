PLUGINS = {}


def register(name, fn):
    PLUGINS[name] = fn
