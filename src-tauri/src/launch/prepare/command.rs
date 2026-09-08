use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub(super) struct JavaLaunchDetails {
    pub(super) executable: OsString,
    pub(super) jvm_args: Vec<String>,
    pub(super) application_args: Vec<String>,
    pub(super) main_class: Option<String>,
}

pub(super) fn java_launch_details(command: &str) -> Option<JavaLaunchDetails> {
    let tokens = command_tokens(command);
    let java_index = java_command_index(&tokens)?;
    let executable = OsString::from(&tokens[java_index]);
    let mut jvm_args = Vec::new();
    let mut index = java_index + 1;
    let mut main_class = None;
    while index < tokens.len() {
        let token = &tokens[index];
        if matches!(
            token.as_str(),
            "-cp"
                | "-classpath"
                | "--class-path"
                | "-p"
                | "--module-path"
                | "--upgrade-module-path"
        ) {
            index = (index + 2).min(tokens.len());
            continue;
        }
        if matches!(token.as_str(), "-jar" | "--module" | "-m") {
            // A jar/module launch has no stable application class for exec-maven-plugin. The
            // executable and JDK are still useful to a caller that only needs preparation.
            break;
        }
        if token.starts_with('-') {
            jvm_args.push(token.clone());
            if java_option_takes_value(token) && index + 1 < tokens.len() {
                jvm_args.push(tokens[index + 1].clone());
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        main_class = Some(token.clone());
        index += 1;
        break;
    }
    let application_args = if main_class.is_some() {
        tokens[index..].to_vec()
    } else {
        Vec::new()
    };
    Some(JavaLaunchDetails {
        executable,
        jvm_args,
        application_args,
        main_class,
    })
}

fn command_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escaped = false;
    let mut started = false;
    let mut characters = command.chars().peekable();

    while let Some(character) = characters.next() {
        if escaped {
            current.push(character);
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' && !in_single_quote {
            if characters
                .peek()
                .is_some_and(|next| next.is_whitespace() || matches!(next, '\\' | '\'' | '"'))
            {
                escaped = true;
            } else {
                // Preserve ordinary backslashes so quoted Windows paths are not corrupted when
                // a saved command is inspected on another platform.
                current.push(character);
            }
            started = true;
            continue;
        }
        match character {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                started = true;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                started = true;
            }
            character if character.is_whitespace() && !in_single_quote && !in_double_quote => {
                if started {
                    tokens.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            character => {
                current.push(character);
                started = true;
            }
        }
    }
    if escaped {
        current.push('\\');
    }
    if started {
        tokens.push(current);
    }
    tokens
}

fn java_command_index(tokens: &[String]) -> Option<usize> {
    tokens.iter().position(|token| {
        Path::new(token)
            .file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|name| matches!(name.to_ascii_lowercase().as_str(), "java" | "java.exe"))
    })
}

pub(super) fn command_environment(command: &str) -> Vec<(OsString, OsString)> {
    let tokens = command_tokens(command);
    let prefix_end = java_command_index(&tokens).unwrap_or_else(|| environment_prefix_end(&tokens));
    if prefix_end == 0 {
        return Vec::new();
    }

    let mut assignments = Vec::new();
    let mut env_command_seen = false;
    for token in &tokens[..prefix_end] {
        if is_env_command(token) && !env_command_seen {
            env_command_seen = true;
            continue;
        }
        let Some((name, value)) = environment_assignment(token) else {
            // Only a contiguous assignment prefix is safe to replay without a shell. An
            // unsupported env option or wrapper command must not cause later arbitrary tokens
            // to be reinterpreted as environment variables.
            return Vec::new();
        };
        if let Some(existing) = assignments.iter_mut().find(|(key, _)| key == &name) {
            existing.1 = value;
        } else {
            assignments.push((name, value));
        }
    }
    assignments
}

fn environment_prefix_end(tokens: &[String]) -> usize {
    let mut index = if tokens.first().is_some_and(|token| is_env_command(token)) {
        1
    } else {
        0
    };
    while tokens
        .get(index)
        .is_some_and(|token| environment_assignment(token).is_some())
    {
        index += 1;
    }
    index
}

fn environment_assignment(token: &str) -> Option<(OsString, OsString)> {
    let (name, value) = token.split_once('=')?;
    if !is_environment_name(name) {
        return None;
    }
    Some((name.into(), value.into()))
}

fn is_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn is_env_command(token: &str) -> bool {
    Path::new(token)
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| matches!(name.to_ascii_lowercase().as_str(), "env" | "env.exe"))
}

pub(super) fn command_active_profiles(command: &str) -> Vec<String> {
    let tokens = command_tokens(command);
    let application_profiles = application_active_profiles(&tokens);
    let jvm_profiles = jvm_active_profiles(&tokens);
    let environment_profiles = command_environment(command)
        .into_iter()
        .find(|(name, _)| name == "SPRING_PROFILES_ACTIVE")
        .and_then(|(_, value)| profile_values(&value.to_string_lossy()));

    // Spring Boot gives command-line application arguments precedence over system properties,
    // which in turn take precedence over environment variables. Preserve that order when a
    // legacy launch command contains more than one way to select a profile.
    application_profiles
        .or(jvm_profiles)
        .or(environment_profiles)
        .unwrap_or_default()
}

fn application_active_profiles(tokens: &[String]) -> Option<Vec<String>> {
    let mut profiles = None;
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if let Some(value) = token.strip_prefix("--spring.profiles.active=") {
            profiles = profile_values(value);
        } else if token == "--spring.profiles.active" {
            if let Some(value) = tokens
                .get(index + 1)
                .filter(|value| !value.starts_with('-'))
            {
                profiles = profile_values(value);
                index += 1;
            }
        } else if token == "--args" {
            if let Some(value) = tokens.get(index + 1) {
                if let Some(nested) = application_active_profiles(&command_tokens(value)) {
                    profiles = Some(nested);
                }
                index += 1;
            }
        } else if let Some(value) = token.strip_prefix("--args=") {
            if let Some(nested) = application_active_profiles(&command_tokens(value)) {
                profiles = Some(nested);
            }
        }
        index += 1;
    }
    profiles
}

fn jvm_active_profiles(tokens: &[String]) -> Option<Vec<String>> {
    tokens
        .iter()
        .filter_map(|token| token.strip_prefix("-Dspring.profiles.active="))
        .filter_map(profile_values)
        .last()
}

fn profile_values(value: &str) -> Option<Vec<String>> {
    let profiles = value
        .split(',')
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!profiles.is_empty()).then_some(profiles)
}

fn java_option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "-agentlib"
            | "-javaagent"
            | "-splash"
            | "--add-exports"
            | "--add-opens"
            | "--add-reads"
            | "--patch-module"
            | "--limit-modules"
            | "--add-modules"
    )
}

pub(super) fn java_home_from_command(command: &str) -> Option<PathBuf> {
    let tokens = command_tokens(command);
    let java_index = java_command_index(&tokens);
    let from_command = java_index
        .and_then(|index| java_home_for(&tokens, index, Path::new(&tokens[index])))
        .or_else(|| {
            tokens.iter().find_map(|token| {
                let value = token.strip_prefix("JAVA_HOME=")?;
                let home = PathBuf::from(value.trim_matches(['\'', '"']));
                home.is_dir().then_some(home)
            })
        });
    from_command
}

fn java_home_for(tokens: &[String], java_index: usize, executable: &Path) -> Option<PathBuf> {
    if let Some(home) = tokens[..java_index].iter().find_map(|token| {
        let value = token.strip_prefix("JAVA_HOME=")?;
        let home = PathBuf::from(value.trim_matches(['\'', '"']));
        home.is_dir().then_some(home)
    }) {
        return Some(home);
    }
    let executable = if executable.is_absolute() {
        executable.to_path_buf()
    } else {
        return None;
    };
    let bin = executable.parent()?;
    if bin.file_name() == Some(OsStr::new("bin")) {
        bin.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

pub(super) fn command_identity(command: &str) -> Vec<String> {
    let mut tokens = command.split_whitespace();
    let first = tokens.find(|token| {
        !token.starts_with('-')
            && !token.contains('=')
            && !matches!(*token, "env" | "nohup" | "exec" | "sudo")
    });
    let Some(first) = first else {
        return Vec::new();
    };
    vec![Path::new(first)
        .file_name()
        .unwrap_or_else(|| OsStr::new(first))
        .to_string_lossy()
        .to_lowercase()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_environment_preserves_only_a_leading_assignment_prefix() {
        let command = r#"env R_HOME="/opt/R with spaces" LD_LIBRARY_PATH='/opt/lib' SPRING_PROFILES_ACTIVE=dev java -cp app.jar App"#;
        assert_eq!(
            command_environment(command),
            vec![
                ("R_HOME".into(), "/opt/R with spaces".into()),
                ("LD_LIBRARY_PATH".into(), "/opt/lib".into()),
                ("SPRING_PROFILES_ACTIVE".into(), "dev".into()),
            ]
        );
        assert_eq!(
            command_environment("R_HOME=/opt/R SPRING_PROFILES_ACTIVE=dev java -cp app.jar App"),
            vec![
                ("R_HOME".into(), "/opt/R".into()),
                ("SPRING_PROFILES_ACTIVE".into(), "dev".into()),
            ]
        );
        assert_eq!(
            command_environment("env SPRING_PROFILES_ACTIVE=dev ./gradlew bootRun"),
            vec![("SPRING_PROFILES_ACTIVE".into(), "dev".into())]
        );
        assert!(command_environment("env -i R_HOME=/opt/R java -cp app.jar App").is_empty());
        assert!(command_environment("wrapper --set R_HOME=/opt/R java -cp app.jar App").is_empty());
    }

    #[test]
    fn command_tokens_preserve_windows_path_separators() {
        assert_eq!(
            command_tokens(r#""C:\Program Files\Java\bin\java.exe" -cp app.jar App"#)[0],
            r#"C:\Program Files\Java\bin\java.exe"#
        );
    }

    #[test]
    fn command_environment_derives_spring_profiles() {
        assert_eq!(
            command_active_profiles("env SPRING_PROFILES_ACTIVE=dev,test java -cp app.jar App"),
            vec!["dev", "test"]
        );
    }

    #[test]
    fn command_active_profiles_derives_application_and_jvm_options() {
        assert_eq!(
            command_active_profiles("./gradlew bootRun --args='--spring.profiles.active=dev'"),
            vec!["dev"]
        );
        assert_eq!(
            command_active_profiles("env SPRING_PROFILES_ACTIVE=dev ./gradlew bootRun"),
            vec!["dev"]
        );
        assert_eq!(
            command_active_profiles("java -cp app.jar App --spring.profiles.active dev,test"),
            vec!["dev", "test"]
        );
        assert_eq!(
            command_active_profiles("java -Dspring.profiles.active=staging -cp app.jar App"),
            vec!["staging"]
        );
    }

    #[test]
    fn command_active_profiles_prefers_application_args_over_jvm_and_environment() {
        let command = "env SPRING_PROFILES_ACTIVE=prod java -Dspring.profiles.active=staging \
            -cp app.jar App --spring.profiles.active=dev";
        assert_eq!(command_active_profiles(command), vec!["dev"]);

        assert_eq!(
            command_active_profiles(
                "env SPRING_PROFILES_ACTIVE=prod java -Dspring.profiles.active=staging \
                    -cp app.jar App"
            ),
            vec!["staging"]
        );
    }
}
