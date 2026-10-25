#!/usr/bin/env python3
"""Deterministic fake app-server. No inference or external I/O."""
import json
import os
import sys
import threading
import time

mode = os.environ.get('FAKE_MODE', 'simple')
with open(os.environ['FAKE_PID_FILE'], 'w') as handle:
    handle.write(str(os.getpid()))
lock = threading.Lock()

def audit(value):
    with open(os.environ['FAKE_AUDIT_FILE'], 'a') as handle:
        handle.write(json.dumps(value) + '\n')

def emit(value):
    with lock:
        print(json.dumps(value), flush=True)

def notify(method, params):
    emit({'method': method, 'params': params})

def turn(status):
    notify('turn/started' if status == 'inProgress' else 'turn/completed',
           {'threadId': 'fake-thread', 'turn': {'id': 'fake-turn', 'status': status, 'items': [], 'error': None}})

def item(status):
    return {'id': 'fake-call', 'type': 'commandExecution', 'command': 'python3 -c "print(42)"',
            'cwd': '/scratch', 'status': status, 'commandActions': [{'type': 'unknown', 'command': 'python3'}]}

def flood():
    for _ in range(10000):
        notify('item/agentMessage/delta', {'itemId': 'fake-message', 'delta': 'x' * 512})

audit({'processArgs': sys.argv[1:]})

if mode == 'startup-hang':
    time.sleep(120)
if mode == 'startup-fail':
    print('simulated startup failure', file=sys.stderr, flush=True)
    sys.exit(1)
for line in sys.stdin:
    value = json.loads(line)
    audit(value)
    method = value.get('method')
    rid = value.get('id')
    if method == 'initialize':
        emit({'id': rid, 'result': {'userAgent': 'fake/0.163.0'}})
    elif method in ('thread/start', 'thread/resume'):
        emit({'id': rid, 'result': {'thread': {'id': 'fake-thread'}, 'model': 'fake-model'}})
        if mode == 'write-pressure':
            # Fill stdout before reading a large turn/start from stdin.
            time.sleep(0.05)
            for _ in range(100):
                notify('future/pressure', {'text':'x'*8192})
    elif method == 'turn/start':
        emit({'id': rid, 'result': {'turn': {'id': 'fake-turn', 'status': 'inProgress'}}})
        if mode == 'delayed-turn':
            time.sleep(0.1)
        turn('inProgress')
        if mode in ('flood', 'ignore-eof', 'flood-ask', 'flood-exit'):
            if mode == 'flood-ask':
                notify('item/started', {'item': item('inProgress')})
                emit({'id': 77, 'method': 'item/commandExecution/requestApproval', 'params': {'itemId':'fake-call','command':'python3','reason':'needs permission'}})
                time.sleep(0.05)
            threading.Thread(target=flood, daemon=True).start()
            if mode == 'flood-exit':
                threading.Timer(0.5, lambda: os._exit(7)).start()
        elif mode == 'mcp':
            emit({'id':77,'method':'mcpServer/elicitation/request','params':{'serverName':'fixture-mcp','threadId':'fake-thread','mode':'url','elicitationId':'fixture-elicitation','url':'https://example.invalid/approve','message':'Approve MCP action'}})
        elif mode in ('gate', 'stall-gate', 'delayed-turn'):
            notify('item/started', {'item': item('inProgress')})
            emit({'id': 77, 'method': 'item/commandExecution/requestApproval', 'params': {
                'itemId': 'fake-call', 'threadId': 'fake-thread', 'turnId': 'fake-turn', 'command': 'python3', 'cwd': '/scratch', 'startedAtMs': 0}})
        else:
            notify('item/agentMessage/delta', {'itemId': 'fake-message', 'delta': 'hello'})
            notify('item/completed', {'item': {'id': 'fake-message', 'type': 'agentMessage', 'text': 'hello'}})
            turn('completed')
    elif method == 'turn/interrupt':
        if mode in ('flood', 'ignore-eof', 'flood-ask', 'flood-exit'):
            # Audit proves Interrupt was consumed even while stdout is blocked.
            continue
        emit({'id': rid, 'result': {}})
        notify('serverRequest/resolved', {'threadId': 'fake-thread', 'requestId': 77})
        turn('interrupted')
    elif rid == 77 and 'result' in value:
        notify('serverRequest/resolved', {'threadId': 'fake-thread', 'requestId': 77})
        if mode != 'mcp':
            notify('item/completed', {'item': item('completed' if value['result'].get('decision') == 'accept' else 'declined')})
        turn('completed')
if mode == 'ignore-eof':
    while True:
        time.sleep(1)
os._exit(0)
