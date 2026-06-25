import os


def purge_temp(tmpdir):
    for name in os.listdir(tmpdir):
        os.remove(os.path.join(tmpdir, name))
