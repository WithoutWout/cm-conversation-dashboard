const base = require("./package.json").build

module.exports = {
  ...base,
  mac: {
    ...base.mac,
    notarize: process.env.APPLE_TEAM_ID
      ? { teamId: process.env.APPLE_TEAM_ID }
      : false,
  },
}
