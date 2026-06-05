def download(request):
    name = request.args["file"]
    return open("/srv/data/" + name).read()
