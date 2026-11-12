## Specification for Keyman-CLI (ASD-STE100 Standard)

**1. General Description**
Keyman-CLI is a command-line software tool. This tool reads environment variables from one master YAML file. The tool writes these variables into a `.env` file for a specific project directory.

**2. System Requirements**

* You must install Python 3.
* You must install the PyYAML library.
* Your operating system must be macOS or Linux.

**3. Data Structure**
The master configuration file must use the YAML (`.yml`) format.

* The top level of the YAML file must contain project names.
* Each project name must contain a list of keys and values.

**Example YAML structure:**

```yaml
project-a:
  DATABASE_URL: "postgres://user:pass@localhost:5432/db_a"
  API_KEY: "secret_key_123"

project-b:
  DEBUG: "true"

```

**4. Commands**

### 4.1. The LOAD Command

This command saves the location of the master YAML file to the system.

**Syntax:**
`keyman load <file_path>`

**Procedure:**

1. Write the command `keyman load` and the file path of your YAML file.
2. Push the Enter key.
3. The tool finds the YAML file at the `<file_path>`.
4. The tool writes the absolute file path into a hidden file (`~/.keyman_config`).
5. The tool shows a success message.

**Error Conditions for LOAD:**

* If the tool does not find the YAML file, the tool shows an error message. The operation stops.

### 4.2. The INJECT Command

This command writes the correct environment variables into a project directory. You must operate the `load` command before you use the `inject` command.

**Syntax:**
`keyman inject <project_name> <target_directory>`

**Procedure:**

1. Write the command `keyman inject`.
2. Write the name of the project.
3. Write the path to the target directory.
4. Push the Enter key.
5. The tool reads the hidden configuration file to find the master YAML file.
6. The tool finds the `<project_name>` in the master YAML file.
7. The tool reads the variables for that project.
8. The tool makes a new file with the name `.env` in the `<target_directory>`.
9. The tool writes the variables into the `.env` file.
10. The tool shows a success message.

**Error Conditions for INJECT:**

* If the tool does not find the hidden configuration file, the tool shows an error message.
* If the tool does not find the `<project_name>` in the YAML file, the tool shows an error message.
* If the `<target_directory>` does not exist, the tool shows an error message.

**5. Security Instructions**

* Do not put the master YAML file in a public directory.
* Set the file permissions of the master YAML file to `600`. This gives read and write access to the owner only.
* Write `.env` in your global `.gitignore` file. This prevents the transmission of secrets to public repositories.## Specification for Keyman-CLI (ASD-STE100 Standard)

**1. General Description**
Keyman-CLI is a command-line software tool. This tool reads environment variables from one master YAML file. The tool writes these variables into a `.env` file for a specific project directory.

**2. System Requirements**

* You must install Python 3.
* You must install the PyYAML library.
* Your operating system must be macOS or Linux.

**3. Data Structure**
The master configuration file must use the YAML (`.yml`) format.

* The top level of the YAML file must contain project names.
* Each project name must contain a list of keys and values.

**Example YAML structure:**

```yaml
project-a:
  DATABASE_URL: "postgres://user:pass@localhost:5432/db_a"
  API_KEY: "secret_key_123"

project-b:
  DEBUG: "true"

```

**4. Commands**

### 4.1. The LOAD Command

This command saves the location of the master YAML file to the system.

**Syntax:**
`keyman load <file_path>`

**Procedure:**

1. Write the command `keyman load` and the file path of your YAML file.
2. Push the Enter key.
3. The tool finds the YAML file at the `<file_path>`.
4. The tool writes the absolute file path into a hidden file (`~/.keyman_config`).
5. The tool shows a success message.

**Error Conditions for LOAD:**

* If the tool does not find the YAML file, the tool shows an error message. The operation stops.

### 4.2. The INJECT Command

This command writes the correct environment variables into a project directory. You must operate the `load` command before you use the `inject` command.

**Syntax:**
`keyman inject <project_name> <target_directory>`

**Procedure:**

1. Write the command `keyman inject`.
2. Write the name of the project.
3. Write the path to the target directory.
4. Push the Enter key.
5. The tool reads the hidden configuration file to find the master YAML file.
6. The tool finds the `<project_name>` in the master YAML file.
7. The tool reads the variables for that project.
8. The tool makes a new file with the name `.env` in the `<target_directory>`.
9. The tool writes the variables into the `.env` file.
10. The tool shows a success message.

**Error Conditions for INJECT:**

* If the tool does not find the hidden configuration file, the tool shows an error message.
* If the tool does not find the `<project_name>` in the YAML file, the tool shows an error message.
* If the `<target_directory>` does not exist, the tool shows an error message.

**5. Security Instructions**

* Do not put the master YAML file in a public directory.
* Set the file permissions of the master YAML file to `600`. This gives read and write access to the owner only.
* Write `.env` in your global `.gitignore` file. This prevents the transmission of secrets to public repositories.
