const { execFileSync } = require('child_process');

function gitSha() {
  return execFileSync('git', ['rev-parse', '--short', 'HEAD']).toString().trim();
}

module.exports = { version: gitSha() };
