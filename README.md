# Why?

Innogames provides an API for grepolis (https://grepolis.com). This API is used in my [GrepolisMap](https://github.com/Turun/GrepolisMap) program. I want to provide that program in the browser. The browser honors [CORS](https://en.wikipedia.org/wiki/Cross-origin_resource_sharing) directives and prevents my program, if it is running in the browser, from making requests to the grepolis API. But requests to my own server are allowed. So this program provides a proxy for the required endpoints (and caching, we wanna be nice to innogames for making the API accessible (CORS aside)).
