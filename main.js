const { app, BrowserWindow, ipcMain, shell, dialog } = require("electron")
const path = require("path")
const fs = require("fs")
const { autoUpdater } = require("electron-updater")

autoUpdater.autoDownload = false
autoUpdater.autoInstallOnAppQuit = false

let win

function dataDir() {
  return app.getPath("userData")
}

function findFile(pattern) {
  // Prefer userData (writable in packaged app), fall back to __dirname (dev)
  for (const dir of [dataDir(), __dirname]) {
    try {
      const files = fs.readdirSync(dir)
      const match = files.find((f) => f.includes(pattern) && f.endsWith(".json"))
      if (match) return path.join(dir, match)
    } catch (_) {}
  }
  return null
}

ipcMain.handle("open-url", (_e, url) => {
  if (/^https?:\/\//.test(url)) shell.openExternal(url)
})

ipcMain.handle("import-file", async (_e, type) => {
  if (type !== "ArticlesExport" && type !== "DialogsExport") {
    return { ok: false, reason: "Invalid type" }
  }
  const { canceled, filePaths } = await dialog.showOpenDialog(win, {
    title: `Select ${type} JSON file`,
    filters: [{ name: "JSON Files", extensions: ["json"] }],
    properties: ["openFile"],
  })
  if (canceled || !filePaths.length) return { ok: false, reason: "canceled" }
  const src = filePaths[0]
  const basename = path.basename(src)
  const destBasename = basename.includes(type) ? basename : type + "_" + basename
  const destDir = dataDir()
  const oldPath = findFile(type)
  if (oldPath && path.basename(oldPath) !== destBasename) {
    try { fs.unlinkSync(oldPath) } catch (_) {}
  }
  fs.copyFileSync(src, path.join(destDir, destBasename))
  return { ok: true, filename: destBasename }
})

ipcMain.handle("get-data", () => {
  const result = { articles: [], dialogs: [], tDialogs: [], files: {} }

  const articlesPath = findFile("ArticlesExport")
  if (articlesPath) {
    const data = JSON.parse(fs.readFileSync(articlesPath, "utf-8"))
    result.articles = data.Articles ?? []
    result.files.articles = path.basename(articlesPath)
  }

  const dialogsPath = findFile("DialogsExport")
  if (dialogsPath) {
    const data = JSON.parse(fs.readFileSync(dialogsPath, "utf-8"))
    result.dialogs = data.dialogs?.result ?? []
    result.tDialogs = Array.isArray(data.tDialogs)
      ? data.tDialogs
      : data.tDialogs?.result ?? []
    result.files.dialogs = path.basename(dialogsPath)
  }

  return result
})

ipcMain.handle("check-for-updates", async () => {
  if (!app.isPackaged) return { status: "dev" }
  try {
    const result = await autoUpdater.checkForUpdates()
    if (!result) return { status: "up-to-date" }
    const latest = result.updateInfo.version
    const current = app.getVersion()
    if (latest === current) return { status: "up-to-date" }
    return { status: "available", version: latest }
  } catch (err) {
    return { status: "error", message: err.message }
  }
})

ipcMain.handle("get-version", () => app.getVersion())

function createWindow() {
  win = new BrowserWindow({
    width: 1200,
    height: 800,
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, "preload.js"),
    },
  })

  win.loadFile("index.html")

  // Uncomment to open DevTools:
  // win.webContents.openDevTools()
}

app.whenReady().then(() => {
  createWindow()

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0) createWindow()
  })
})

app.on("window-all-closed", () => {
  if (process.platform !== "darwin") app.quit()
})
