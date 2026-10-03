import React, { lazy, Suspense } from 'react'
import ReactDOM from 'react-dom/client'
import { beacon, installErrorReporter } from './errorReporter'
import './styles.css'

// Install the reporter before loading either window's interface.
installErrorReporter()
const App=lazy(()=>import('./App'))
const Settings=lazy(()=>import('./Settings'))
const TrayDashboard=lazy(()=>import('./TrayDashboard'))

const root = document.getElementById('root')
if (!root) {
  beacon('fatal: #root missing from index.html')
  throw new Error('#root missing from index.html')
}

// One HTML entry, two independently loaded interfaces. The floating assistant
// need not load settings or token-history controls when it starts.
const isSettings = new URLSearchParams(window.location.search).get('view') === 'settings'
const isUsage = new URLSearchParams(window.location.search).get('view') === 'usage'
if(isUsage) document.documentElement.classList.add('tray-view')
if (isSettings) document.documentElement.classList.add('settings-view')

ReactDOM.createRoot(root).render(
  <React.StrictMode><Suspense fallback={isSettings?<p role="status">正在读取设置…</p>:null}>{isSettings ? <Settings /> : isUsage ? <TrayDashboard /> : <App />}</Suspense></React.StrictMode>,
)
