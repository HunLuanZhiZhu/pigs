"""BFCL V4 多轮样本的可读中文时间线。

这只是语言层的解释：不能把工具“请求调用”写成“操作成功”。
所有原始英文用户输入、确切调用及执行回包仍保留于报告的技术附录。
"""
from __future__ import annotations
import collections
import html
import re

TOOL_ZH = {
    'get_zipcode_based_on_city':'查询城市邮编','search_messages':'搜索消息','displayCarStatus':'查看车辆状态',
    'ls':'列出文件','startEngine':'启动发动机','retrieve_invoice':'查询发票',
    'get_nearest_airport_by_city':'查找城市附近的机场','cd':'切换文件夹','get_flight_cost':'查询机票价格',
    'get_booking_history':'查看预订记录','get_user_id':'查询用户编号',
    'get_credit_card_balance':'查询信用卡余额','travel_get_login_status':'查看旅行账户登录状态',
    'cat':'查看文件内容','authenticate_travel':'登录旅行账户','estimate_distance':'计算路程距离',
    'view_messages_sent':'查看已发送的消息','get_all_credit_cards':'查看已登记信用卡',
    'pwd':'查看当前文件夹','lockDoors':'锁定或解锁车门','pressBrakePedal':'踩下刹车',
    'estimate_drive_feasibility_by_mileage':'判断车辆能否完成该段路程','post_tweet':'发布推文',
    'book_flight':'预订航班','find':'搜索文件','releaseBrakePedal':'松开刹车',
    'get_order_details':'查询订单详情','message_get_login_status':'查看消息账户登录状态',
    'get_budget_fiscal_year':'查询本年度预算','compute_exchange_rate':'换算货币',
    'fillFuelTank':'加油','list_users':'查看用户列表','get_stock_info':'查询股票行情',
    'check_tire_pressure':'检查轮胎气压','list_all_airports':'查看机场列表',
    'posting_get_login_status':'查看发文账户登录状态','set_budget_limit':'设置预算上限',
    'wc':'统计文件字数或行数','get_watchlist':'查看自选股名单',
    'get_message_stats':'查看消息统计','get_account_info':'查看交易账户信息',
    'get_user_tweets':'查看已发布推文','mv':'移动文件或改名',
    'send_message':'发送消息','get_order_history':'查看历史订单',
    'echo':'向文件写入文字','message_login':'登录消息账户',
    'contact_customer_support':'联系客户支持','touch':'创建文件',
    'get_tweet':'查看指定推文','get_current_time':'查询当前时间',
    'ticket_get_login_status':'查询工单账户登录状态',
    'get_symbol_by_name':'查询股票代码','get_user_tickets':'查看客服工单',
    'place_order':'下股票订单','purchase_insurance':'购买保险',
    'authenticate_twitter':'登录发文账户','find_nearest_tire_shop':'寻找附近的轮胎维修店',
    'get_ticket':'查看客服工单','create_ticket':'创建客服工单',
    'grep':'查找文件中的文字','liter_to_gallon':'将升换算成加仑',
    'add_to_watchlist':'把股票加入自选股','activateParkingBrake':'操作驻车刹车',
    'cp':'复制文件','comment':'发表评论','gallon_to_liter':'将加仑换算成升',
    'cancel_booking':'取消机票预订','rm':'删除文件','tail':'查看文件末尾',
    'get_current_speed':'查询当前车速','trading_get_login_status':'查看股票账户登录状态',
    'cancel_order':'取消股票订单','get_tweet_comments':'查看推文评论',
    'mean':'计算平均值','set_navigation':'设置导航目的地',
    'get_available_stocks':'查看指定行业有哪些股票',
    'list_all_following':'查看关注的账号','delete_message':'删除消息',
    'ticket_login':'登录客服工单账户','verify_traveler_information':'核对旅客资料',
    'close_ticket':'关闭客服工单','get_transaction_history':'查看交易记录',
    'diff':'比较两个文件','retweet':'转发推文','update_market_status':'更新市场状态',
    'setCruiseControl':'设置定速巡航','sort':'给内容排序','mention':'在推文中提及用户',
    'register_credit_card':'登记信用卡','du':'查看文件占用空间','search_tweets':'搜索推文',
    'get_user_stats':'查看用户统计','get_outside_temperature_from_google':'查询室外气温',
    'get_outside_temperature_from_weather_com':'查询室外天气',
    'mkdir':'创建文件夹','withdraw_funds':'从交易账户取款',
    'resolve_ticket':'解决客服工单','subtract':'计算减法',
    'round_number':'将数字取整','logarithm':'计算对数',
    'remove_stock_from_watchlist':'从自选股删除股票','fund_account':'给股票账户充值',
    'rmdir':'删除空文件夹',
}

TRIVIAL = {'please','could','would','kindly','me','you','i','the','and','to','of','with','a','an','for','in'}

def chinese_intent(message: str) -> str:
    """只判断可以从用户原话中识别的主旨；不编造金额、姓名、工具执行结果。"""
    s = (message or '').lower()
    if not s.strip():
        return '这一轮的用户文字没有完整保存，不能据此还原具体要求'
    if re.search(r'\b(sector is|the sector:|sector\s*=\s*)',s):
        m=re.search(r'(?:sector is|the sector:|sector\s*=\s*)\s*([a-z]+)',s)
        return '补充要查询的行业'+('：'+{'technology':'科技','healthcare':'医疗','finance':'金融','energy':'能源'}.get(m.group(1),m.group(1)) if m else '')
    if re.search(r'\b(card id|credit card id)\b',s) and len(s)<165:
        return '补充付款时要使用的信用卡编号'
    if re.search(r'\b(share|shares)\b',s) and len(s)<115 and not any(k in s for k in ('cancel','order history')):
        number=re.search(r'\b(\d[\d,]*)\s+shares?\b',s)
        return '明确要交易的股票数量'+('：'+number.group(1)+' 股' if number else '')
    if re.search(r'\b(hashtag|hashtags|tag\b|mention|mentions|friend|buddy)\b',s) and any(k in s for k in ('tweet','twitter','post','tag','mention')):
        return '说明推文内容、标签或要提及的账号'
    if re.search(r'\b(cancel|cancellation)\b',s):
        if any(x in s for x in ('flight','booking','ticket','itinerary','reservation')):
            return '要求取消之前的航班预订'
        if any(x in s for x in ('stock','trade','order')):
            return '要求取消之前的股票订单'
    if re.search(r'\b(invoice|invoicing)\b',s):
        if any(x in s for x in ('ticket','support','complaint','discrepancy')):
            return '要求处理发票问题或创建客服工单'
        return '要求查询或核对机票发票'
    if any(x in s for x in ('support ticket','customer support','create a ticket','ticket id')):
        return '要求处理客服问题或工单'
    if re.search(r'\b(insurance|insured|coverage|cover)\b',s):
        return '讨论或明确旅行保险的保障方案与金额'
    if any(x in s for x in ('credit card balance','card balance','remaining balance')):
        return '要求查询信用卡余额'
    if re.search(r'\b(register|add|save)\b',s) and 'credit card' in s:
        return '要求登记新的信用卡'
    if re.search(r'\b(budget|spending cap|limit my travel|expense limit)\b',s):
        return '要求确认旅行预算或设置花费上限'
    if any(x in s for x in ('exchange rate','convert ','currency','rmb','euros','usd')) and not any(x in s for x in ('flight','budget')):
        return '要求核对金额或进行货币换算'
    if any(x in s for x in ('book a flight','book the flight','book a ticket','book a business','reserve a flight','flight reservation')):
        return '要求预订指定的航班'
    if any(x in s for x in ('flight cost','ticket price','cost of the flight','fare','nearest airport')):
        return '要求查询机场或机票价格'
    if 'flight' in s or 'travel itinerary' in s or 'traveler' in s:
        return '进一步确认航班、旅客或预订事项'
    if 'watchlist' in s:
        if any(x in s for x in ('remove','take','out of','delete')):
            return '要求将指定股票从自选名单移除'
        if any(x in s for x in ('add','append','include','put','insert')):
            return '要求将指定股票加入自选名单'
        return '要求查看自选股名单'
    if any(x in s for x in ('stock price','share price','stock details','stock information','stock info','current price')):
        return '要求了解股票价格或行情'
    if re.search(r'\b(stocks|stock)\b',s) and ('sector' in s or 'technology' in s):
        return '要求查找指定行业的股票'
    if re.search(r'\b(place an order|purchase \d+|buy \d+|shares of|invest|acquire \d+)\b',s) and any(x in s for x in ('share','stock','portfolio','order','nvda','aapl','invest')):
        num=re.search(r'\b(\d[\d,]*)\s+shares?\b',s)
        return '要求购买股票'+('，数量为 '+num.group(1)+' 股' if num else '')
    if any(x in s for x in ('order details','most recent order','latest order','order history','status of the order','orders i')):
        return '要求核对股票订单或交易记录'
    if any(x in s for x in ('fund my account','deposit','add funds','trading funds','allocate an extra')):
        return '要求给交易账户充值'
    if any(x in s for x in ('withdraw','cash out','take out $')):
        return '要求从交易账户取钱'
    if any(x in s for x in ('market open','market status','market hours')):
        return '要求查询股票市场是否开市'
    if 'tweet' in s or 'twitter' in s or 'retweet' in s:
        if any(x in s for x in ('retweet','repost')):
            return '要求转发指定推文'
        return '要求发布、查看或修改推文'
    if any(x in s for x in ('sent messages','messages i sent','messages sent','sent by me','outbox')):
        return '要求查看自己已经发出的消息'
    if any(x in s for x in ('send a message','send the message','message to','notify my','inform my')):
        return '要求向指定的人发送消息'
    if any(x in s for x in ('inbox','messages','message')):
        return '要求查看、搜索或管理消息'
    if any(x in s for x in ('engine','ignition','start the car','start my car')):
        return '要求完成发动机启动相关的车辆操作'
    if any(x in s for x in ('brake pedal','brake','parking brake')):
        return '要求按指定方式使用脚刹或驻车刹车'
    if any(x in s for x in ('doors','lock my car','lock the car','unlock')):
        return '要求锁车或查看车门状态'
    if any(x in s for x in ('tire','tyre')):
        if any(x in s for x in ('navigation','navigate','repair','shop')):
            return '要求检查轮胎并导航到维修店'
        return '要求检查轮胎气压'
    if any(x in s for x in ('navigation','navigate','drive to','route to')):
        return '要求规划路线或设置导航'
    if any(x in s for x in ('distance','mileage','drive feasibility','drive from','drive between')):
        return '要求计算路程并判断能否驾车到达'
    if any(x in s for x in ('fuel','tank','gallon','liter','litre')):
        return '要求检查油量、加油或换算燃油单位'
    if any(x in s for x in ('car','vehicle','driving','speed','cruise')):
        return '要求查看车辆状态或完成驾驶准备'
    if any(x in s for x in ('directory','folder','archive','move the file','copy the file','rename','file name','files','file','terminal')):
        if any(x in s for x in ('delete','remove','clear','clean up')):
            return '要求清理或删除指定文件'
        if any(x in s for x in ('copy','move','archive','rename')):
            return '要求将文件整理到指定文件夹，必要时改名'
        if any(x in s for x in ('sort','average','mean','words','count','length')):
            return '要求读取文件并进行排序或统计'
        if any(x in s for x in ('create','write','put the answer','append')):
            return '要求创建文件或写入内容'
        return '要求查看文件或文件夹中的内容'
    if any(x in s for x in ('average','mean','calculate','logarithm','subtract')):
        return '要求完成指定数值计算'
    if any(x in s for x in ('login','log in','sign in','authenticate','access my account')):
        return '要求登录或确认账户身份'
    if len(s.strip())<=80:
        return '用户补充了前面任务的一项条件或说明'
    return '用户在前面要求的基础上提出新的条件或下一项任务（精确原话保存在技术记录中）'

def phase_zh(phase: str) -> str:
    return {'Pre':'前期判断环节','Executor':'主要执行环节','Post':'事后检查环节'}.get(phase,'未知内部环节')

def tool_zh(tool: str) -> str:
    if tool in TOOL_ZH:
        return TOOL_ZH[tool]
    # 未知函数不能视为已知成功操作。 / Do not treat unknown functions as confirmed successful operations.
    return '一项尚未归类的操作'

def action_summaries(steps: list[dict], max_items: int=12) -> tuple[list[str], int]:
    actions=[tool_zh(name) for s in steps for name in s.get('tools',[]) if name]
    if not actions:
        return [],0
    runs=[]
    for label in actions:
        if runs and runs[-1][0]==label:
            runs[-1]=(label,runs[-1][1]+1)
        else:
            runs.append((label,1))
    final=[name+('（连续 '+str(cnt)+' 次）' if cnt>1 else '') for name,cnt in runs[:max_items]]
    return final, max(0,len(runs)-max_items)

def chronological_turns(x: dict, steps: list[dict], esc) -> str:
    grouped: dict[int,list[dict]]=collections.defaultdict(list)
    for st in steps:
        if isinstance(st.get('turn'),int):
            grouped[st['turn']].append(st)
    all_turns=max(len(x.get('user_turns') or []),max(grouped,default=-1)+1)
    rows=[]
    first_error=(x.get('first_error') or {}).get('turn')
    for i in range(all_turns):
        user=(x.get('user_turns') or [])
        msg=user[i] if i<len(user) else ''
        intent=chinese_intent(msg)
        p=grouped.get(i,[])
        names,remaining=action_summaries(p)
        if p:
            obs=' → '.join(names)
            if remaining:obs+=' → 还有 '+str(remaining)+' 种连续操作（详见技术记录）'
        else:
            obs='正式执行记录没有这一轮的工具操作；可能是在等用户补充信息，也可能是没有完成该做的事。'
        caution='这是实际尝试执行的动作顺序，不能仅凭名称确定每次都成功。'
        highlight=' <span class="turn-alert">官方第一次发现状态不符就在这一次要求结束后</span>' if i==first_error else ''
        rows.append(
            '<li class="turn-story"><strong>第 '+str(i+1)+' 次用户提出要求：'+esc(intent)+'</strong>'+highlight+
            '<div class="turn-ops"><b>接下来 AI 做了：</b> '+esc(obs)+'</div>'+
            '<div class="turn-caution">'+caution+'</div></li>'
        )
    return '<ol class="turn-list">'+''.join(rows)+'</ol>'
