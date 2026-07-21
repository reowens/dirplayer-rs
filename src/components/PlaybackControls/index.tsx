import { useEffect, useState } from 'react'
import { faPlay, faStop, faRotateBack } from '@fortawesome/free-solid-svg-icons'
import IconButton from '../IconButton'
import styles from './styles.module.css'
import { play, stop, reset } from 'vm-rust'
import { isElectron } from '../../utils/electron'
import { isMcpEnabled, setMcpEnabled, getMcpPort, setMcpPort, getMcpStatus, subscribeMcpStatus } from '../../mcp'

function McpToggle() {
  const [enabled, setEnabled] = useState(() => isMcpEnabled());
  const [port, setPort] = useState(() => getMcpPort());
  const [editingPort, setEditingPort] = useState(false);
  const [portDraft, setPortDraft] = useState(String(port));
  const [status, setStatus] = useState(() => getMcpStatus());
  const [copied, setCopied] = useState(false);

  useEffect(() => subscribeMcpStatus(setStatus), []);

  const commitPort = () => {
    const parsed = parseInt(portDraft, 10);
    if (parsed > 0 && parsed <= 65535) {
      setPort(parsed);
      setMcpPort(parsed);
    } else {
      setPortDraft(String(port));
    }
    setEditingPort(false);
  };

  const isListening = status.state === 'listening';
  const isStarting = status.state === 'starting';
  const isBusy = isStarting || status.restartRequired === true;
  const label = status.state === 'listening'
    ? 'MCP ON'
    : status.state === 'starting'
      ? 'MCP STARTING'
      : status.restartRequired
        ? 'MCP RESTART REQUIRED'
        : status.state === 'error'
          ? 'MCP ERROR'
          : 'MCP OFF';

  return (
    <div className={styles.mcpContainer}>
      <button
        className={isListening ? styles.mcpButtonActive : styles.mcpButton}
        disabled={isBusy}
        onClick={async () => {
          const next = !enabled;
          setEnabled(next);
          await setMcpEnabled(next);
        }}
        title={status.state === 'error' ? status.message : isListening ? 'MCP server is listening. Click to stop.' : 'Start MCP server for AI debugging tools'}
      >
        {label}
      </button>
      {(isListening || isStarting) && (
        editingPort ? (
          <input
            className={styles.mcpPortInput}
            value={portDraft}
            onChange={(e) => setPortDraft(e.target.value)}
            onBlur={commitPort}
            onKeyDown={(e) => { if (e.key === 'Enter') commitPort(); if (e.key === 'Escape') { setPortDraft(String(port)); setEditingPort(false); } }}
            autoFocus
            size={5}
          />
        ) : (
          <span
            className={styles.mcpUrl}
            onClick={() => { setPortDraft(String(port)); setEditingPort(true); }}
            title="Click to change port"
          >
            http://127.0.0.1:{status.port || port}
          </span>
        )
      )}
      {isListening && status.token && (
        <button
          className={styles.mcpTokenButton}
          onClick={async () => {
            await navigator.clipboard.writeText(status.token || '');
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1500);
          }}
          title="Copy the run-scoped MCP bearer token"
        >
          {copied ? 'TOKEN COPIED' : 'COPY TOKEN'}
        </button>
      )}
      {status.state === 'error' && <span className={styles.mcpError}>{status.message}</span>}
    </div>
  );
}

export default function PlaybackControls() {
  return <div className={styles.container}>
    <IconButton icon={faPlay} onClick={() => { play() }} />
    <IconButton icon={faStop} onClick={() => { stop() }} />
    <IconButton icon={faRotateBack} onClick={() => { reset() }} />
    {isElectron() && <>
      <div className={styles.spacer} />
      <McpToggle />
    </>}
  </div>
}
