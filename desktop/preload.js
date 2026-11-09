'use strict';
// The only things the page can ask the app to do.
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('guardianDesktop', {
  // A new approval request arrived: bring Guardian to the front.
  request: () => ipcRenderer.send('guardian:request'),
  show: () => ipcRenderer.send('guardian:show'),
  version: () => ipcRenderer.invoke('guardian:version'),
});
