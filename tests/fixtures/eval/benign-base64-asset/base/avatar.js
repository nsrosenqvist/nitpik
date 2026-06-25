function dataUri(bytes, mime) {
  return 'data:' + mime + ';base64,';
}

module.exports = { dataUri };
