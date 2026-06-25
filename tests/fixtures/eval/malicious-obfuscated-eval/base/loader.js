function loadConfig(name) {
  return require('./config/' + name + '.json');
}

module.exports = { loadConfig };
