const { contextBridge, ipcRenderer } = require("electron")

contextBridge.exposeInMainWorld("electronAPI", {
  getData: () => ipcRenderer.invoke("get-data"),
  openUrl: (url) => ipcRenderer.invoke("open-url", url),
  importFile: (type) => ipcRenderer.invoke("import-file", type),
  checkForUpdates: () => ipcRenderer.invoke("check-for-updates"),
  getVersion: () => ipcRenderer.invoke("get-version"),
})
