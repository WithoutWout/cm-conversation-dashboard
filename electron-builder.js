const base = require("./package.json").build

const config = { ...base }

if (process.env.APPLE_TEAM_ID) {
  config.mac = { ...base.mac, notarize: { teamId: process.env.APPLE_TEAM_ID } }
}

module.exports = config
