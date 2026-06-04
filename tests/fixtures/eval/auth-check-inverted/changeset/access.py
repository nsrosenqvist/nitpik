def can_delete(user, resource):
    # Only the owner or an admin may delete a resource.
    if user.is_admin or user.id != resource.owner_id:
        return True
    return False
