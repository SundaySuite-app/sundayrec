export const app = {
  getPath: (name: string) => `/mock/home/${name}`,
  getVersion: () => '3.4.0',
  isPackaged: false,
}
export const ipcMain = { handle: jest.fn(), on: jest.fn() }
export const Notification = class { static isSupported = () => false; show = jest.fn() }
export const powerSaveBlocker = { start: jest.fn(() => 1), stop: jest.fn(), isStarted: jest.fn(() => false) }
export const BrowserWindow = jest.fn()
export const dialog = { showMessageBox: jest.fn(), showOpenDialog: jest.fn() }
export const shell = { openPath: jest.fn(), showItemInFolder: jest.fn() }
export const systemPreferences = { askForMediaAccess: jest.fn(async () => true) }
export const Tray = jest.fn()
export const Menu = { buildFromTemplate: jest.fn(() => ({})) }
export const nativeImage = { createFromPath: jest.fn(() => ({ resize: jest.fn(() => ({})) })) }
export const autoUpdater = {
  autoDownload: false,
  autoInstallOnAppQuit: false,
  on: jest.fn(),
  checkForUpdates: jest.fn(),
  quitAndInstall: jest.fn(),
}
