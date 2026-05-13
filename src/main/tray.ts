import { Tray, Menu, nativeImage, app } from 'electron'
import type { BrowserWindow } from 'electron'
import path from 'path'

let tray: Tray | null = null
let win: BrowserWindow | null = null
let isRecording = false
let hasError    = false

export function create(mainWindow: BrowserWindow): void {
  win = mainWindow

  const iconFile = process.platform === 'darwin' ? 'tray-idleTemplate.png' : 'tray-idle.png'
  const iconPath = path.join(__dirname, '../../assets', iconFile)
  let icon = nativeImage.createFromPath(iconPath)
  if (process.platform === 'darwin') icon = icon.resize({ width: 18, height: 18 })
  tray = new Tray(icon)
  tray.setToolTip('SundayRec — kjører i bakgrunnen')

  tray.on('click', () => {
    if (!win) return
    if (win.isVisible()) win.focus()
    else { win.show(); win.focus() }
  })

  updateMenu()
}

function updateMenu(): void {
  if (!tray) return

  const label = isRecording
    ? '🔴 Tar opp…'
    : hasError
    ? '⚠️ Feil — klikk for detaljer'
    : '✅ Klar'

  const menu = Menu.buildFromTemplate([
    { label, enabled: false },
    { type: 'separator' },
    {
      label: 'Åpne SundayRec',
      click: () => { win?.show(); win?.focus() }
    },
    { type: 'separator' },
    {
      label: isRecording ? 'Stopp opptak' : 'Start opptak nå',
      click: () => {
        if (!win) return
        if (isRecording) {
          win.webContents.send('tray-stop-recording')
        } else {
          win.show()
          win.webContents.send('tray-start-recording')
        }
      }
    },
    { type: 'separator' },
    { label: 'Avslutt', click: () => app.quit() }
  ])

  tray.setContextMenu(menu)

  const suffix = process.platform === 'darwin' ? 'Template.png' : '.png'
  const base   = isRecording ? 'tray-recording' : hasError ? 'tray-error' : 'tray-idle'
  try {
    let icon = nativeImage.createFromPath(path.join(__dirname, '../../assets', base + suffix))
    if (process.platform === 'darwin') icon = icon.resize({ width: 18, height: 18 })
    tray.setImage(icon)
  } catch {}
}

export function setRecording(active: boolean): void {
  isRecording = active
  if (active) hasError = false
  updateMenu()
}

export function setError(active: boolean): void {
  hasError = active
  updateMenu()
}
