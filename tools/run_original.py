"""Run unmodified original backend with config/log paths redirected to a test directory."""
import ast
from pathlib import Path
import sys
import types
source=Path(sys.argv[1]);directory=Path(sys.argv[2]);port=int(sys.argv[3])
sys.dont_write_bytecode=True;sys.path.insert(0,str(source/'backend'))
def load(name,excluded):
    path=source/'backend'/f'{name}.py';tree=ast.parse(path.read_text(encoding='utf-8'))
    tree.body=[n for n in tree.body if not(isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id in excluded for t in n.targets))]
    module=types.ModuleType(name);module.__file__=str(path);sys.modules[name]=module;exec(compile(tree,str(path),'exec'),module.__dict__);return module
cfg=load('config',{'config_manager'});cfg.get_config_dir=lambda:directory;cfg.config_manager=cfg.ConfigManager()
log=load('logger',{'gateway_logger','logger'});log.get_log_dir=lambda:directory/'logs';log.gateway_logger=log.GatewayLogger();log.logger=log.gateway_logger.get_logger()
import main
import uvicorn
uvicorn.run(main.app,host='127.0.0.1',port=port,log_config=None)
