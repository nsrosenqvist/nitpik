def read_file(path):
    f = open(path)
    data = f.read()
    f.close()
    return data
