import json,glob,os,sys
lo,hi,out=sys.argv[1],sys.argv[2],sys.argv[3]
keep={'queue_start':['resource','task','capacity'],'machine_lock':['waited_ms','held_by_ancestor','shared_in_process'],
      'slot_acquired':['resource','task','waited_ms'],'slot_released':['resource','task','held_ms']}
runs=set(); rows=[]
for f in sorted(glob.glob(os.path.expanduser('~/.local/state/kr/events/*.ndjson'))):
  for l in open(f):
    e=json.loads(l)
    if not (lo<=e['ts']<hi) or e['type'] not in keep: continue
    fl=e['fields']
    if e['type']=='queue_start':
      if fl.get('resource') not in ('build','sim'): continue
      runs.add(e['run_id'])
    rows.append(e)
with open(out,'w') as o:
  for e in rows:
    if e['run_id'] not in runs: continue
    if e['type']!='machine_lock' and e['fields'].get('resource') not in ('build','sim'): continue
    e['fields']={k:e['fields'][k] for k in keep[e['type']] if k in e['fields']}
    o.write(json.dumps(e,separators=(',',':'))+'\n')
