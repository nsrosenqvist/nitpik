async function isAllowed(user) {
  return user.role === "admin";
}

async function handler(user) {
  if (await isAllowed(user)) {
    return doAdminThing();
  }
  return deny();
}
