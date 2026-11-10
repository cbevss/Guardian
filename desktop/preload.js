'use strict';
// The only things the page can ask the app to do.
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('guardianDesktop', {
  // A new approval request arrived: bring Guardian to the front.
  request: () => ipcRenderer.send('guardian:request'),
  show: () => ipcRenderer.send('guardian:show'),
  version: () => ipcRenderer.invoke('guardian:version'),
  appFolder: () => ipcRenderer.invoke('guardian:appFolder'),
  openAppFolder: () => ipcRenderer.send('guardian:openAppFolder'),
  updates: {
    get: () => ipcRenderer.invoke('update:get'),
    setAuto: on => ipcRenderer.invoke('update:setAuto', on),
    check: () => ipcRenderer.invoke('update:check'),
    install: () => ipcRenderer.invoke('update:install'),
    onState: cb => ipcRenderer.on('update:state', (_e, s) => cb(s)),
  },
  serverBase: () => ipcRenderer.invoke('guardian:server'),
  // Lock screen, reset emails and passkeys:
  sec: (name, ...args) => ipcRenderer.invoke('sec:' + name, ...args),
  // Custom title bar buttons:
  win: {
    min: () => ipcRenderer.send('win:min'),
    max: () => ipcRenderer.send('win:max'),
    close: () => ipcRenderer.send('win:close'),
    isMax: () => ipcRenderer.invoke('win:isMax'),
    onMax: cb => ipcRenderer.on('guardian:maximized', (_e, v) => cb(v)),
  },
});
