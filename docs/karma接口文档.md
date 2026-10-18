---
title: 默认模块
language_tabs:
  - shell: Shell
  - http: HTTP
  - javascript: JavaScript
  - ruby: Ruby
  - python: Python
  - php: PHP
  - java: Java
  - go: Go
toc_footers: []
includes: []
search: true
code_clipboard: true
highlight_theme: darkula
headingLevel: 2
generator: "@tarslib/widdershins v4.0.30"

---

# 默认模块

Base URLs:

# Authentication

# Default

## POST 用户登录

POST /v1/auth/login

> Body 请求参数

```json
{
    "username": "xuhaoxuan",
    "password": "XuHaoXuan@2025!"
}
```

### 请求参数

|名称|位置|类型|必选|说明|
|---|---|---|---|---|
|body|body|object| 是 |none|
|» username|body|string| 是 |用户名|
|» password|body|string| 是 |密码|

> 返回示例

> 200 Response

```json
{
  "success": true,
  "token": "string",
  "secret": "string",
  "userId": "string"
}
```

### 返回结果

|状态码|状态码含义|说明|数据模型|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|none|Inline|

### 返回数据结构

状态码 **200**

|名称|类型|必选|约束|中文名|说明|
|---|---|---|---|---|---|
|» success|boolean|true|none||消息状态|
|» token|string|true|none||用户token|
|» secret|string|true|none||Happy 凭据的一部分，普通 REST 请求不会把它放进 Header|
|» userId|string|true|none||用户id|

## GET 用户信息

GET /v1/account/profile

### 请求参数

|名称|位置|类型|必选|说明|
|---|---|---|---|---|
|Authorization|header|string| 是 |none|

> 返回示例

> 200 Response

```json
{
  "id": "string",
  "timestamp": 0,
  "firstName": null,
  "lastName": null,
  "username": "string",
  "labAdmin": true,
  "envBadge": "string",
  "avatar": null,
  "github": null,
  "connectedServices": [
    "string"
  ]
}
```

### 返回结果

|状态码|状态码含义|说明|数据模型|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|none|Inline|

### 返回数据结构

状态码 **200**

|名称|类型|必选|约束|中文名|说明|
|---|---|---|---|---|---|
|» id|string|true|none||none|
|» timestamp|integer|true|none||none|
|» firstName|null|true|none||姓氏|
|» lastName|null|true|none||名字|
|» username|string|true|none||用户账号|
|» labAdmin|boolean|true|none||none|
|» envBadge|string|true|none||none|
|» avatar|null|true|none||头像|
|» github|null|true|none||none|
|» connectedServices|[string]|true|none||none|

# 数据模型
