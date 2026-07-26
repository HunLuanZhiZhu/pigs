import re

with open(r'd:/AIWorkSpace/pigs/docs/upstream-requests.log', encoding='utf-8') as f:
    all_lines = f.readlines()

# #6 body: lines 12596-15266 (0-indexed: 12595-15265)
# #7 body: lines 15267-end (0-indexed: 15266-end)
body6 = ''.join(all_lines[12595:15266])
body7 = ''.join(all_lines[15266:])

def analyze(label, body):
    m = re.search(r'"messages"\s*:\s*\[', body)
    if not m:
        print(f'{label}: no messages field')
        return
    start = m.end()
    depth = 1
    i = start
    while i < len(body) and depth > 0:
        if body[i] == '[': depth += 1
        elif body[i] == ']': depth -= 1
        i += 1
    msg_str = body[start:i-1]
    roles = re.findall(r'"role"\s*:\s*"(\w+)"', msg_str)
    print(f'=== {label}: {len(roles)} messages ===')
    print(f'  roles: {roles}')

    # 找最后3条消息的 role 和 content 预览
    # 简单按 {"role": 拆分
    parts = re.split(r'(\{"role":\s*"\w+")', msg_str)
    # 重新组合
    msgs_raw = []
    for j in range(1, len(parts), 2):
        if j+1 < len(parts):
            msgs_raw.append(parts[j] + parts[j+1])

    print(f'  最后3条:')
    for m_raw in msgs_raw[-3:]:
        role_m = re.search(r'"role":\s*"(\w+)"', m_raw)
        role = role_m.group(1) if role_m else '?'
        # 找 content 预览
        content_m = re.search(r'"content":\s*"([^"]{0,80})', m_raw)
        preview = content_m.group(1) if content_m else '(no content)'
        # 找 tool_call_id
        tcid_m = re.search(r'"tool_call_id":\s*"([^"]+)"', m_raw)
        tcid = f' [tool_call_id: {tcid_m.group(1)[:25]}]' if tcid_m else ''
        print(f'    role={role}{tcid}')
        print(f'      预览: {preview}...')
    print()

analyze('#6', body6)
analyze('#7', body7)
