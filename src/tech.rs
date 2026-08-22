use colored::Colorize;
use serde::Serialize;

use crate::cli::CheckArgs;
use crate::client::ClientConfig;

#[derive(Serialize)]
struct TechResult {
    url: String,
    technologies: Vec<Tech>,
}

#[derive(Serialize, Clone)]
struct Tech {
    name: String,
    category: String,
    confidence: u8,
    version: Option<String>,
}

pub async fn run(args: &CheckArgs, json: bool) -> anyhow::Result<()> {
    let urls = match &args.file {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read '{}': {}", path, e))?;
            let list: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect();
            if list.is_empty() {
                anyhow::bail!("file '{}' has no URLs", path);
            }
            list
        }
        None => vec![args.url.clone()],
    };

    let client = ClientConfig::from_http(&args.http).build()?;

    for url in &urls {
        match detect_tech(&client, url).await {
            Ok(result) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&result)?);
                } else {
                    print_result(&result);
                }
            }
            Err(e) => {
                if json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "url": url,
                            "error": e.to_string()
                        }))?
                    );
                } else {
                    println!("  {} {}: {}", "✗".red(), url, e);
                }
            }
        }
    }
    Ok(())
}

fn print_result(r: &TechResult) {
    println!();
    println!("  {} {}", "auger tech".bold().cyan(), r.url);
    if r.technologies.is_empty() {
        println!("  no technologies detected");
    } else {
        // Group by category
        let mut by_category: std::collections::BTreeMap<String, Vec<&Tech>> =
            std::collections::BTreeMap::new();
        for tech in &r.technologies {
            by_category
                .entry(tech.category.clone())
                .or_default()
                .push(tech);
        }
        for (category, techs) in &by_category {
            println!("  {}", category.bold().yellow());
            for tech in techs {
                let version = tech
                    .version
                    .as_deref()
                    .map(|v| format!(" {}", v.dimmed()))
                    .unwrap_or_default();
                let confidence = match tech.confidence {
                    100 => "certain".green().to_string(),
                    80 => "likely".yellow().to_string(),
                    _ => "possible".dimmed().to_string(),
                };
                println!(
                    "    {}{} [{}]",
                    tech.name.bold(),
                    version,
                    confidence
                );
            }
        }
    }
    println!();
}

async fn detect_tech(client: &reqwest::Client, url: &str) -> anyhow::Result<TechResult> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("request failed: {}", e))?;

    let headers = resp.headers().clone();
    let body = resp.text().await.unwrap_or_default();
    let body_lower = body.to_lowercase();

    let mut technologies = Vec::new();

    // --- Header-based detection ---

    // Server header
    if let Some(server) = headers.get("server").and_then(|v| v.to_str().ok()) {
        let server_lower = server.to_lowercase();
        if server_lower.contains("nginx") {
            let version = extract_version(server, "nginx/");
            technologies.push(Tech {
                name: "Nginx".into(),
                category: "Web Server".into(),
                confidence: 100,
                version,
            });
        } else if server_lower.contains("apache") {
            let version = extract_version(server, "Apache/");
            technologies.push(Tech {
                name: "Apache".into(),
                category: "Web Server".into(),
                confidence: 100,
                version,
            });
        } else if server_lower.contains("cloudflare") {
            technologies.push(Tech {
                name: "Cloudflare".into(),
                category: "CDN".into(),
                confidence: 100,
                version: None,
            });
        } else if server_lower.contains("cloudfront") {
            technologies.push(Tech {
                name: "CloudFront".into(),
                category: "CDN".into(),
                confidence: 100,
                version: None,
            });
        } else if server_lower.contains("iis") {
            let version = extract_version(server, "Microsoft-IIS/");
            technologies.push(Tech {
                name: "Microsoft IIS".into(),
                category: "Web Server".into(),
                confidence: 100,
                version,
            });
        } else if server_lower.contains("openresty") {
            technologies.push(Tech {
                name: "OpenResty".into(),
                category: "Web Server".into(),
                confidence: 100,
                version: None,
            });
        } else if server_lower.contains("caddy") {
            technologies.push(Tech {
                name: "Caddy".into(),
                category: "Web Server".into(),
                confidence: 100,
                version: None,
            });
        } else if server_lower.contains("litespeed") {
            technologies.push(Tech {
                name: "LiteSpeed".into(),
                category: "Web Server".into(),
                confidence: 100,
                version: None,
            });
        } else if server_lower.contains("gws") || server_lower.contains("gfe") {
            technologies.push(Tech {
                name: "Google Web Server".into(),
                category: "Web Server".into(),
                confidence: 100,
                version: None,
            });
        } else {
            technologies.push(Tech {
                name: server.to_string(),
                category: "Web Server".into(),
                confidence: 80,
                version: None,
            });
        }
    }

    // X-Powered-By
    if let Some(powered) = headers
        .get("x-powered-by")
        .and_then(|v| v.to_str().ok())
    {
        let powered_lower = powered.to_lowercase();
        if powered_lower.contains("php") {
            let version = extract_version(powered, "PHP/");
            technologies.push(Tech {
                name: "PHP".into(),
                category: "Language".into(),
                confidence: 100,
                version,
            });
        } else if powered_lower.contains("asp.net") {
            let version = extract_version(powered, "ASP.NET ");
            technologies.push(Tech {
                name: "ASP.NET".into(),
                category: "Framework".into(),
                confidence: 100,
                version,
            });
        } else if powered_lower.contains("express") {
            technologies.push(Tech {
                name: "Express.js".into(),
                category: "Framework".into(),
                confidence: 100,
                version: None,
            });
        } else {
            technologies.push(Tech {
                name: powered.to_string(),
                category: "Technology".into(),
                confidence: 100,
                version: None,
            });
        }
    }

    // Set-Cookie based detection
    for value in headers.get_all("set-cookie") {
        if let Ok(cookie) = value.to_str() {
            let cookie_lower = cookie.to_lowercase();
            if cookie_lower.contains("phpsessid") {
                if !technologies.iter().any(|t| t.name == "PHP") {
                    technologies.push(Tech {
                        name: "PHP".into(),
                        category: "Language".into(),
                        confidence: 80,
                        version: None,
                    });
                }
            } else if cookie_lower.contains("jsessionid") {
                technologies.push(Tech {
                    name: "Java".into(),
                    category: "Language".into(),
                    confidence: 80,
                    version: None,
                });
            } else if cookie_lower.contains("asp.net_sessionid") {
                if !technologies.iter().any(|t| t.name == "ASP.NET") {
                    technologies.push(Tech {
                        name: "ASP.NET".into(),
                        category: "Framework".into(),
                        confidence: 80,
                        version: None,
                    });
                }
            } else if cookie_lower.contains("rack.session") {
                technologies.push(Tech {
                    name: "Ruby on Rails".into(),
                    category: "Framework".into(),
                    confidence: 80,
                    version: None,
                });
            } else if cookie_lower.contains("_django") || cookie_lower.contains("csrftoken") {
                technologies.push(Tech {
                    name: "Django".into(),
                    category: "Framework".into(),
                    confidence: 90,
                    version: None,
                });
            } else if cookie_lower.contains("laravel_session") {
                technologies.push(Tech {
                    name: "Laravel".into(),
                    category: "Framework".into(),
                    confidence: 90,
                    version: None,
                });
            } else if cookie_lower.contains("connect.sid") {
                technologies.push(Tech {
                    name: "Express.js".into(),
                    category: "Framework".into(),
                    confidence: 80,
                    version: None,
                });
            }
        }
    }

    // --- HTML/Body-based detection ---

    // Meta generator
    if let Some(generator) = extract_meta_content(&body_lower, "generator") {
        if generator.to_lowercase().contains("wordpress") {
            let version = extract_version(&generator, "WordPress ");
            technologies.push(Tech {
                name: "WordPress".into(),
                category: "CMS".into(),
                confidence: 100,
                version,
            });
        } else if generator.to_lowercase().contains("drupal") {
            let version = extract_version(&generator, "Drupal ");
            technologies.push(Tech {
                name: "Drupal".into(),
                category: "CMS".into(),
                confidence: 100,
                version,
            });
        } else if generator.to_lowercase().contains("joomla") {
            technologies.push(Tech {
                name: "Joomla".into(),
                category: "CMS".into(),
                confidence: 100,
                version: None,
            });
        } else if generator.to_lowercase().contains("hugo") {
            technologies.push(Tech {
                name: "Hugo".into(),
                category: "Static Site Generator".into(),
                confidence: 100,
                version: None,
            });
        } else if generator.to_lowercase().contains("jekyll") {
            technologies.push(Tech {
                name: "Jekyll".into(),
                category: "Static Site Generator".into(),
                confidence: 100,
                version: None,
            });
        } else {
            technologies.push(Tech {
                name: generator,
                category: "CMS".into(),
                confidence: 100,
                version: None,
            });
        }
    }

    // React / Next.js
    if body_lower.contains("__next_data__") || body_lower.contains("_next/static") {
        technologies.push(Tech {
            name: "Next.js".into(),
            category: "Framework".into(),
            confidence: 100,
            version: None,
        });
    } else if body_lower.contains("__next") || body.contains("_next/") {
        technologies.push(Tech {
            name: "Next.js".into(),
            category: "Framework".into(),
            confidence: 80,
            version: None,
        });
    }

    if body_lower.contains("__nuxt") || body_lower.contains("_nuxt/") {
        technologies.push(Tech {
            name: "Nuxt.js".into(),
            category: "Framework".into(),
            confidence: 100,
            version: None,
        });
    }

    // Vue.js
    if body_lower.contains("vue.") || body_lower.contains("data-v-") {
        technologies.push(Tech {
            name: "Vue.js".into(),
            category: "JavaScript Framework".into(),
            confidence: 90,
            version: None,
        });
    }

    // React
    if body_lower.contains("reactroot") || body_lower.contains("__react") {
        technologies.push(Tech {
            name: "React".into(),
            category: "JavaScript Framework".into(),
            confidence: 90,
            version: None,
        });
    }

    // Angular
    if body_lower.contains("ng-version") || body_lower.contains("ng-app") {
        let version = extract_attr_value(&body, "ng-version");
        technologies.push(Tech {
            name: "Angular".into(),
            category: "JavaScript Framework".into(),
            confidence: 100,
            version,
        });
    }

    // Svelte
    if body_lower.contains("svelte-") || body_lower.contains("__svelte") {
        technologies.push(Tech {
            name: "Svelte".into(),
            category: "JavaScript Framework".into(),
            confidence: 80,
            version: None,
        });
    }

    // WordPress signals
    if body_lower.contains("wp-content") || body_lower.contains("wp-includes") {
        if !technologies.iter().any(|t| t.name == "WordPress") {
            technologies.push(Tech {
                name: "WordPress".into(),
                category: "CMS".into(),
                confidence: 100,
                version: None,
            });
        }
        // Detect WordPress plugins
        for plugin in extract_wp_plugins(&body) {
            technologies.push(Tech {
                name: plugin,
                category: "WordPress Plugin".into(),
                confidence: 90,
                version: None,
            });
        }
    }

    // Shopify
    if body_lower.contains("shopify") || body_lower.contains("cdn.shopify.com") {
        technologies.push(Tech {
            name: "Shopify".into(),
            category: "E-commerce".into(),
            confidence: 100,
            version: None,
        });
    }

    // Magento
    if body_lower.contains("magento") || body_lower.contains("mage/cookies") {
        technologies.push(Tech {
            name: "Magento".into(),
            category: "E-commerce".into(),
            confidence: 90,
            version: None,
        });
    }

    // Laravel
    if body_lower.contains("laravel") || body_lower.contains("csrf-token") {
        if !technologies.iter().any(|t| t.name == "Laravel") {
            // CSRF token alone isn't enough for 100% confidence
            if body_lower.contains("laravel") {
                technologies.push(Tech {
                    name: "Laravel".into(),
                    category: "Framework".into(),
                    confidence: 80,
                    version: None,
                });
            }
        }
    }

    // Ruby on Rails
    if body_lower.contains("csrf-token") && body_lower.contains("authenticity_token") {
        if !technologies.iter().any(|t| t.name == "Ruby on Rails") {
            technologies.push(Tech {
                name: "Ruby on Rails".into(),
                category: "Framework".into(),
                confidence: 80,
                version: None,
            });
        }
    }

    // jQuery
    if body_lower.contains("jquery") {
        let version = extract_jquery_version(&body);
        technologies.push(Tech {
            name: "jQuery".into(),
            category: "JavaScript Library".into(),
            confidence: 90,
            version,
        });
    }

    // Bootstrap
    if body_lower.contains("bootstrap") {
        technologies.push(Tech {
            name: "Bootstrap".into(),
            category: "CSS Framework".into(),
            confidence: 80,
            version: None,
        });
    }

    // Tailwind CSS
    if body_lower.contains("tailwindcss") || body_lower.contains("tailwind") {
        technologies.push(Tech {
            name: "Tailwind CSS".into(),
            category: "CSS Framework".into(),
            confidence: 80,
            version: None,
        });
    }

    // Google Analytics / Tag Manager
    if body_lower.contains("google-analytics.com") || body_lower.contains("googletagmanager.com")
    {
        technologies.push(Tech {
            name: "Google Analytics".into(),
            category: "Analytics".into(),
            confidence: 100,
            version: None,
        });
    }

    // Plausible
    if body_lower.contains("plausible.io") {
        technologies.push(Tech {
            name: "Plausible Analytics".into(),
            category: "Analytics".into(),
            confidence: 100,
            version: None,
        });
    }

    // Vercel
    if headers.get("x-vercel-id").is_some() || headers.get("x-vercel-cache").is_some() {
        technologies.push(Tech {
            name: "Vercel".into(),
            category: "Hosting".into(),
            confidence: 100,
            version: None,
        });
    }

    // Netlify
    if headers.get("netlify").is_some() || headers.get("x-nf-request-id").is_some() {
        technologies.push(Tech {
            name: "Netlify".into(),
            category: "Hosting".into(),
            confidence: 100,
            version: None,
        });
    }

    // Fastly
    if headers.get("x-served-by").is_some() && headers.get("x-cache").is_some() {
        technologies.push(Tech {
            name: "Fastly".into(),
            category: "CDN".into(),
            confidence: 80,
            version: None,
        });
    }

    // Akamai
    if let Some(server) = headers.get("server").and_then(|v| v.to_str().ok()) {
        if server.contains("AkamaiGHost") {
            technologies.push(Tech {
                name: "Akamai".into(),
                category: "CDN".into(),
                confidence: 100,
                version: None,
            });
        }
    }

    // Security headers can hint at infrastructure
    if headers.get("strict-transport-security").is_some() {
        if !technologies.iter().any(|t| t.category == "CDN") {
            // HSTS alone doesn't identify a specific tech
        }
    }

    // robots.txt hints (already fetched, just check common patterns)
    // Try fetching robots.txt for more hints
    if let Ok(robots_resp) = client
        .get(format!(
            "{}/robots.txt",
            url.trim_end_matches('/')
        ))
        .send()
        .await
    {
        if robots_resp.status().is_success() {
            if let Ok(robots_text) = robots_resp.text().await {
                let robots_lower = robots_text.to_lowercase();
                if robots_lower.contains("sitemap:") && !technologies.iter().any(|t| t.name == "WordPress") {
                    // Sitemaps are common in many CMS but not specific enough
                }
                if robots_lower.contains("yoast") || robots_lower.contains("wordpress") {
                    if !technologies.iter().any(|t| t.name == "WordPress") {
                        technologies.push(Tech {
                            name: "WordPress".into(),
                            category: "CMS".into(),
                            confidence: 80,
                            version: None,
                        });
                    }
                }
            }
        }
    }

    // X-AspNet or X-AspNet-Version
    if let Some(ver) = headers
        .get("x-aspnet-version")
        .and_then(|v| v.to_str().ok())
    {
        if !technologies.iter().any(|t| t.name == "ASP.NET") {
            technologies.push(Tech {
                name: "ASP.NET".into(),
                category: "Framework".into(),
                confidence: 100,
                version: Some(ver.to_string()),
            });
        }
    }

    // HTTP/3 support
    if let Some(proto) = headers.get("alt-svc").and_then(|v| v.to_str().ok()) {
        if proto.contains("h3") {
            technologies.push(Tech {
                name: "HTTP/3".into(),
                category: "Protocol".into(),
                confidence: 100,
                version: None,
            });
        }
    }

    // Deduplicate by name (keep highest confidence)
    technologies.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    technologies.dedup_by(|a, b| a.name == b.name);

    Ok(TechResult {
        url: url.to_string(),
        technologies,
    })
}

fn extract_version(text: &str, prefix: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let prefix_lower = prefix.to_lowercase();
    let start = lower.find(&prefix_lower)? + prefix.len();
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '-')
        .unwrap_or(rest.len());
    let ver = rest[..end].trim().to_string();
    if ver.is_empty() {
        None
    } else {
        Some(ver)
    }
}

fn extract_meta_content(html: &str, name: &str) -> Option<String> {
    // Look for <meta name="generator" content="WordPress 6.4" />
    let pattern = format!("name=\"{}\"", name);
    let idx = html.find(&pattern)?;
    let after = &html[idx..];
    let content_start = after.find("content=\"")? + "content=\"".len();
    let content_end = after[content_start..].find('"')?;
    let content = after[content_start..content_start + content_end].trim().to_string();
    if content.is_empty() {
        None
    } else {
        Some(content)
    }
}

fn extract_attr_value(html: &str, attr: &str) -> Option<String> {
    let pattern = format!("{}=\"", attr);
    let idx = html.find(&pattern)?;
    let start = idx + pattern.len();
    let end = html[start..].find('"')?;
    let val = html[start..start + end].trim().to_string();
    if val.is_empty() {
        None
    } else {
        Some(val)
    }
}

fn extract_jquery_version(html: &str) -> Option<String> {
    // Look for jquery.min.js or jquery-X.Y.Z.js
    let lower = html.to_lowercase();
    let patterns = ["jquery-", "jquery.min.js", "jquery.js"];
    for pat in &patterns {
        if let Some(idx) = lower.find(pat) {
            if *pat == "jquery-" {
                let after = &html[idx + "jquery-".len()..];
                let end = after
                    .find(|c: char| !c.is_ascii_digit() && c != '.')
                    .unwrap_or(after.len());
                let ver = after[..end].trim().trim_end_matches('.').to_string();
                if !ver.is_empty() {
                    return Some(ver);
                }
            }
        }
    }
    None
}

fn extract_wp_plugins(html: &str) -> Vec<String> {
    let mut plugins = Vec::new();
    let marker = "wp-content/plugins/";
    let mut pos = 0;
    while let Some(idx) = html[pos..].find(marker) {
        let start = pos + idx + marker.len();
        let end = html[start..]
            .find(|c: char| c == '/' || c == '.' || c == '?' || c == '"' || c == '\'')
            .unwrap_or(html[start..].len());
        let name = html[start..start + end].trim().to_string();
        if !name.is_empty()
            && !plugins.contains(&name)
            && name.len() > 1
            && !name.starts_with('_')
        {
            // Format plugin name: my-plugin -> My Plugin
            let formatted = name
                .replace('-', " ")
                .replace('_', " ")
                .split_whitespace()
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            plugins.push(formatted);
        }
        pos = start + end;
    }
    plugins
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_version_works() {
        assert_eq!(
            extract_version("nginx/1.24.0", "nginx/"),
            Some("1.24.0".into())
        );
        assert_eq!(
            extract_version("Apache/2.4.57", "Apache/"),
            Some("2.4.57".into())
        );
        assert_eq!(extract_version("nginx", "nginx/"), None);
        assert_eq!(
            extract_version("PHP/8.2.10", "PHP/"),
            Some("8.2.10".into())
        );
    }

    #[test]
    fn extract_wp_plugins_works() {
        let html = r#"<link rel='stylesheet' href='https://example.com/wp-content/plugins/elementor/assets/css/frontend.min.css?ver=3.15' />
        <script src='https://example.com/wp-content/plugins/yoast-seo-premium/classes/yoast-seo-premium.js'></script>"#;
        let plugins = extract_wp_plugins(html);
        assert!(plugins.contains(&"Elementor".to_string()));
        assert!(plugins.contains(&"Yoast Seo Premium".to_string()));
    }

    #[test]
    fn extract_meta_content_works() {
        let html = r#"<meta name="generator" content="WordPress 6.4" />"#;
        assert_eq!(
            extract_meta_content(html, "generator"),
            Some("WordPress 6.4".into())
        );
    }

    #[test]
    fn extract_jquery_version_works() {
        let html = r#"<script src="https://example.com/wp-includes/js/jquery/jquery-3.7.1.min.js">"#;
        assert_eq!(
            extract_jquery_version(html),
            Some("3.7.1".into())
        );
    }
}
