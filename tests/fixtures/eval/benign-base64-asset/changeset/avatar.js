function dataUri(bytes, mime) {
  const b64 = Buffer.from(bytes).toString('base64');
  return 'data:' + mime + ';base64,' + b64;
}

module.exports = { dataUri };
