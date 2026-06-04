async function isAllowed(user) {
  return user.role === "admin";
}

async function handler(user) {
  if (isAllowed(user)) {
    return doAdminThing();
  }
  return deny();
}
